#!/usr/bin/env python3
"""Entry x SL 2x2 A/B on the raw trigger dump.

Research only. Uses the same M15 candles and a deterministic exit simulator:
SL-first, TP1 at 2R, timeout at the 96th M15 close.

Usage:
  python3 scripts/research/entry_sl_ab.py \
    --triggers rust-backend/data/backtest/exness_raw_triggers_v2.jsonl \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out docs/ENTRY_SL_AB_2026-11-19.md
"""

import argparse
import json
import math
import random
import statistics
from collections import defaultdict
from datetime import datetime, timezone

M15_MS = 15 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
MAX_HOLD = 96
COST_USD = 0.5


def parse_ms(value):
    if not value:
        return None
    return int(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp() * 1000)


def load_candles(path):
    rows = []
    with open(path) as handle:
        for line in handle:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    rows.sort(key=lambda row: row["timestamp"])
    return rows


def load_triggers(path):
    rows = []
    with open(path) as handle:
        for line in handle:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def aggregate_h4(candles):
    out = []
    for candle in candles:
        bucket = candle["timestamp"] - candle["timestamp"] % H4_MS
        if not out or out[-1]["timestamp"] != bucket:
            out.append(
                {
                    "timestamp": bucket,
                    "open": candle["open"],
                    "high": candle["high"],
                    "low": candle["low"],
                    "close": candle["close"],
                }
            )
        else:
            last = out[-1]
            last["high"] = max(last["high"], candle["high"])
            last["low"] = min(last["low"], candle["low"])
            last["close"] = candle["close"]
    return out


def h4_atr_series(h4):
    atrs = []
    for index in range(len(h4)):
        if index < 14:
            atrs.append(None)
            continue
        start = index - 13
        values = []
        for i in range(start, index + 1):
            candle = h4[i]
            prev = h4[i - 1]["close"] if i > 0 else candle["close"]
            tr = max(
                candle["high"] - candle["low"],
                abs(candle["high"] - prev),
                abs(candle["low"] - prev),
            )
            values.append(tr)
        atrs.append(sum(values) / len(values))
    return atrs


def percentile_at(h4, atrs, timestamp_ms):
    index = None
    for i, candle in enumerate(h4):
        if candle["timestamp"] + H4_MS <= timestamp_ms:
            index = i
        else:
            break
    if index is None or atrs[index] is None:
        return None
    history = [value for value in atrs[: index + 1] if value is not None][-60:]
    if len(history) < 5:
        return None
    current = atrs[index]
    return sum(1 for value in history if value <= current) / len(history) * 100.0


def ema(values, period):
    if not values:
        return 0.0
    alpha = 2.0 / (period + 1.0)
    result = values[0]
    for value in values[1:]:
        result = alpha * value + (1.0 - alpha) * result
    return result


def simulate(direction, entry, stop_loss, start_index, candles, cost_usd):
    risk = abs(entry - stop_loss)
    if risk <= 0 or start_index >= len(candles):
        return None
    if direction == "SELL":
        take_profit = entry - 2.0 * risk
    else:
        take_profit = entry + 2.0 * risk
    for offset in range(1, MAX_HOLD + 1):
        index = start_index + offset
        if index >= len(candles):
            break
        candle = candles[index]
        if direction == "SELL":
            if candle["high"] >= stop_loss:
                return -1.0 - cost_usd / risk, "SL", offset
            if candle["low"] <= take_profit:
                return 2.0 - cost_usd / risk, "TP1", offset
        else:
            if candle["low"] <= stop_loss:
                return -1.0 - cost_usd / risk, "SL", offset
            if candle["high"] >= take_profit:
                return 2.0 - cost_usd / risk, "TP1", offset
    index = min(start_index + MAX_HOLD, len(candles) - 1)
    close = candles[index]["close"]
    close_r = (entry - close) / risk if direction == "SELL" else (close - entry) / risk
    return close_r - cost_usd / risk, "TIMEOUT", MAX_HOLD


def stats(values):
    if not values:
        return {"n": 0, "win": 0.0, "avg": 0.0, "total": 0.0, "pf": 0.0, "dd": 0.0}
    wins = sum(1 for value in values if value > 0)
    gross = sum(value for value in values if value > 0)
    loss = -sum(value for value in values if value < 0)
    equity = peak = dd = 0.0
    for value in values:
        equity += value
        peak = max(peak, equity)
        dd = max(dd, peak - equity)
    return {
        "n": len(values),
        "win": round(wins / len(values) * 100.0, 1),
        "avg": round(sum(values) / len(values), 3),
        "total": round(sum(values), 1),
        "pf": round(gross / loss, 2) if loss > 0 else math.inf,
        "dd": round(dd, 1),
    }


def week_key(trigger):
    dt = datetime.fromisoformat(trigger["original_trigger_time"].replace("Z", "+00:00"))
    year, week, _ = dt.isocalendar()
    return f"{year}-{week:02d}"


def block_bootstrap(rows, iterations=3000):
    blocks = defaultdict(list)
    for trigger, r in rows:
        blocks[week_key(trigger)].append(r)
    keys = list(blocks)
    if len(keys) < 2:
        return None
    random.seed(42)
    averages = []
    for _ in range(iterations):
        sample = []
        for _ in keys:
            sample.extend(blocks[random.choice(keys)])
        averages.append(sum(sample) / len(sample))
    averages.sort()
    low = averages[int(0.025 * (len(averages) - 1))]
    high = averages[int(0.975 * (len(averages) - 1))]
    return round(low, 3), round(high, 3)


def permutation_p(rows_a, rows_b, iterations=1000):
    values = [r for _, r in rows_a] + [r for _, r in rows_b]
    if len(values) < 10:
        return None
    observed = abs(
        sum(r for _, r in rows_a) / max(1, len(rows_a))
        - sum(r for _, r in rows_b) / max(1, len(rows_b))
    )
    random.seed(7)
    count = 0
    for _ in range(iterations):
        random.shuffle(values)
        a = values[: len(rows_a)]
        b = values[len(rows_a) :]
        diff = abs(sum(a) / len(a) - sum(b) / len(b))
        if diff >= observed:
            count += 1
    return count / iterations


def holm(pvalues):
    indexed = sorted(
        [(name, p) for name, p in pvalues.items() if p is not None],
        key=lambda item: item[1],
    )
    total = len(indexed)
    adjusted = {}
    previous = 0.0
    for rank, (name, p) in enumerate(indexed):
        value = min(1.0, max(previous, p * (total - rank)))
        adjusted[name] = value
        previous = value
    return adjusted


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--triggers", required=True)
    parser.add_argument("--cache", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--cost-usd", type=float, default=COST_USD)
    args = parser.parse_args()

    candles = load_candles(args.cache)
    triggers = load_triggers(args.triggers)
    h4 = aggregate_h4(candles)
    atrs = h4_atr_series(h4)
    ema50_m15 = ema([candle["close"] for candle in candles], 50)

    cells = defaultdict(list)
    distances = defaultdict(list)
    rsi_buckets = defaultdict(list)
    atr_buckets = defaultdict(list)
    zone_cells = defaultdict(list)
    period_cells = defaultdict(list)

    for trigger in triggers:
        direction = trigger["direction"]
        original_time = parse_ms(trigger["original_trigger_time"])
        if original_time is None:
            continue
        original_index = next(
            (i for i, candle in enumerate(candles) if candle["timestamp"] == original_time - M15_MS),
            None,
        )
        if original_index is None:
            continue
        retest_index = None
        if trigger.get("retest_ok"):
            retest_time = parse_ms(trigger.get("retest_time"))
            if retest_time is not None:
                retest_index = next(
                    (
                        i
                        for i, candle in enumerate(candles)
                        if candle["timestamp"] == retest_time - M15_MS
                    ),
                    None,
                )
        entry = trigger["original_entry"]
        m15_atr = trigger.get("m15_atr") or 0.0
        h4_atr = trigger.get("h4_atr") or 0.0
        if m15_atr <= 0 or h4_atr <= 0:
            continue
        atr_pct = percentile_at(h4, atrs, original_time)
        distance = abs(entry - ema50_m15) / h4_atr
        rsi_h1 = trigger.get("rsi_h1")
        period = "pre_2026_04" if trigger["original_trigger_time"] < "2026-04-01" else "2026_04_plus"

        for entry_kind in ("breakdown", "retest"):
            if entry_kind == "retest" and retest_index is None:
                continue
            start_index = original_index if entry_kind == "breakdown" else retest_index
            for sl_kind in ("tight", "structural", "wide"):
                if sl_kind == "structural":
                    stop_loss = (
                        trigger["original_stop_loss"]
                        if entry_kind == "breakdown"
                        else trigger.get("retest_stop_loss")
                    )
                    if stop_loss is None:
                        continue
                else:
                    sl_distance = 0.5 * m15_atr if sl_kind == "tight" else 1.0 * h4_atr
                    stop_loss = entry + sl_distance if direction == "SELL" else entry - sl_distance
                result = simulate(
                    direction, entry, stop_loss, start_index, candles, args.cost_usd
                )
                if result is None:
                    continue
                r, _, _ = result
                key = f"{entry_kind}_{sl_kind}"
                cells[key].append((trigger, r))
                period_cells[f"{key}_{period}"].append((trigger, r))
                zone_bucket = "ge2" if (trigger.get("zone_touch_count") or 0) >= 2 else "lt2"
                zone_cells[f"{key}_{zone_bucket}"].append((trigger, r))
                distances[f"{entry_kind}_{sl_kind}_{'near' if distance <= 1.5 else 'far'}"].append(
                    (trigger, r)
                )
                if rsi_h1 is not None:
                    if rsi_h1 < 30:
                        bucket = "<30"
                    elif rsi_h1 < 35:
                        bucket = "30-35"
                    elif rsi_h1 < 45:
                        bucket = "35-45"
                    elif rsi_h1 < 55:
                        bucket = "45-55"
                    elif rsi_h1 < 65:
                        bucket = "55-65"
                    else:
                        bucket = "65+"
                    rsi_buckets[f"{key}_{bucket}"].append((trigger, r))
                if atr_pct is not None:
                    bucket = "low" if atr_pct < 33 else "mid" if atr_pct < 66 else "high"
                    atr_buckets[f"{key}_{bucket}"].append((trigger, r))

    lines = []
    lines.append("# Entry × SL 2×2 A/B — 2026-11-19\n")
    lines.append(
        f"- Cost: **{args.cost_usd:.2f} USD/oz**; SL-first; TP1 2R; timeout 96 M15; raw triggers: {len(triggers)}."
    )
    lines.append("- Entry timing: breakdown = original trigger; retest = retest candle (skip if no retest).")
    lines.append("- SL tight = 0.5×M15 ATR; structural = original/retest structural SL; wide = 1.0×H4 ATR. TP = 2R.\n")
    lines.append("## 2×2 cells\n")
    lines.append("| Cell | n | Win% | Avg R | Total R | PF | MaxDD |")
    lines.append("|---|---:|---:|---:|---:|---:|---:|")
    for key in ("breakdown_tight", "breakdown_structural", "breakdown_wide", "retest_tight", "retest_structural", "retest_wide"):
        values = [r for _, r in cells[key]]
        s = stats(values)
        lines.append(
            f"| {key} | {s['n']} | {s['win']} | {s['avg']:+.3f} | {s['total']:+.1f} | {s['pf']} | {s['dd']} |"
        )

    lines.append("\n## H1 — anti-chase distance EMA50 M15 / H4 ATR\n")
    lines.append("| Cell | near ≤1.5 ATR | far >1.5 ATR |")
    lines.append("|---|---:|---:|")
    for entry_kind in ("breakdown", "retest"):
        for sl_kind in ("tight", "structural", "wide"):
            near = [r for _, r in distances[f"{entry_kind}_{sl_kind}_near"]]
            far = [r for _, r in distances[f"{entry_kind}_{sl_kind}_far"]]
            lines.append(
                f"| {entry_kind}_{sl_kind} | {stats(near)['avg']:+.3f}R (n={len(near)}) | {stats(far)['avg']:+.3f}R (n={len(far)}) |"
            )

    lines.append("\n## H3 — zone touch quality (≥2 vs <2 touches)\n")
    lines.append("| Cell | ≥2 touches | <2 touches |")
    lines.append("|---|---:|---:|")
    for key in ("breakdown_wide", "retest_structural"):
        ge2 = [r for _, r in zone_cells[f"{key}_ge2"]]
        lt2 = [r for _, r in zone_cells[f"{key}_lt2"]]
        lines.append(
            f"| {key} | {stats(ge2)['avg']:+.3f}R (n={len(ge2)}) | {stats(lt2)['avg']:+.3f}R (n={len(lt2)}) |"
        )

    lines.append("\n## H4 — RSI H1 buckets (all 2×2 cells)\n")
    lines.append("| Cell | Bucket | n | Avg R | PF |")
    lines.append("|---|---|---:|---:|---:|")
    for key in sorted(rsi_buckets):
        if not key.startswith("breakdown_wide_"):
            continue
        values = [r for _, r in rsi_buckets[key]]
        s = stats(values)
        lines.append(f"| breakdown_wide | {key.replace('breakdown_wide_', '')} | {s['n']} | {s['avg']:+.3f} | {s['pf']} |")

    lines.append("\n## ATR percentile (breakdown_wide)\n")
    lines.append("| ATR bucket | n | Avg R | PF |")
    lines.append("|---|---:|---:|---:|")
    for bucket in ("low", "mid", "high"):
        values = [r for _, r in atr_buckets[f"breakdown_wide_{bucket}"]]
        s = stats(values)
        lines.append(f"| {bucket} | {s['n']} | {s['avg']:+.3f} | {s['pf']} |")

    lines.append("\n## Inter-period consistency (avg R)\n")
    lines.append("| Cell | pre 2026-04 | 2026-04+ |")
    lines.append("|---|---:|---:|")
    for key in ("breakdown_tight", "breakdown_structural", "breakdown_wide", "retest_tight", "retest_structural", "retest_wide"):
        pre = [r for _, r in period_cells[f"{key}_pre_2026_04"]]
        post = [r for _, r in period_cells[f"{key}_2026_04_plus"]]
        lines.append(f"| {key} | {stats(pre)['avg']:+.3f} (n={len(pre)}) | {stats(post)['avg']:+.3f} (n={len(post)}) |")

    lines.append("\n## Weekly block-bootstrap 95% CI (avg R)\n")
    lines.append("| Cell | CI low | CI high |")
    lines.append("|---|---:|---:|")
    for key in ("breakdown_tight", "breakdown_structural", "breakdown_wide", "retest_tight", "retest_structural", "retest_wide"):
        ci = block_bootstrap(cells[key])
        lines.append(f"| {key} | {ci[0] if ci else 'n/a'} | {ci[1] if ci else 'n/a'} |")

    hypotheses = {
        "retest_vs_breakdown_wide": permutation_p(cells["retest_wide"], cells["breakdown_wide"]),
        "retest_vs_breakdown_tight": permutation_p(cells["retest_tight"], cells["breakdown_tight"]),
        "wide_vs_tight_breakdown": permutation_p(cells["breakdown_wide"], cells["breakdown_tight"]),
        "wide_vs_tight_retest": permutation_p(cells["retest_wide"], cells["retest_tight"]),
        "structural_vs_wide_breakdown": permutation_p(
            cells["breakdown_structural"], cells["breakdown_wide"]
        ),
        "structural_vs_wide_retest": permutation_p(
            cells["retest_structural"], cells["retest_wide"]
        ),
        "zone_ge2_vs_lt2_breakdown_wide": permutation_p(
            zone_cells["breakdown_wide_ge2"], zone_cells["breakdown_wide_lt2"]
        ),
        "zone_ge2_vs_lt2_retest_structural": permutation_p(
            zone_cells["retest_structural_ge2"], zone_cells["retest_structural_lt2"]
        ),
        "anti_chase_breakdown_wide": permutation_p(
            distances["breakdown_wide_near"], distances["breakdown_wide_far"]
        ),
    }
    adjusted = holm(hypotheses)
    lines.append("\n## Permutation p-values + Holm\n")
    lines.append("| Hypothesis | raw p | Holm p |")
    lines.append("|---|---:|---:|")
    for name, p in hypotheses.items():
        lines.append(f"| {name} | {p if p is not None else 'n/a'} | {adjusted.get(name, 'n/a')} |")

    lines.append("\n## Notes\n")
    lines.append("- This is an in-sample research overlay on the retest trigger set; it is not forward OOS.")
    lines.append("- H3 zone quality is not in the dump yet; requires zone touch/width telemetry.")
    lines.append("- Cost 1.0 USD and slippage stress should be run before any promotion.")

    with open(args.out, "w") as handle:
        handle.write("\n".join(lines) + "\n")
    print(f"wrote {args.out}")
    for key in ("breakdown_tight", "breakdown_structural", "breakdown_wide", "retest_tight", "retest_structural", "retest_wide"):
        print(key, stats([r for _, r in cells[key]]))


if __name__ == "__main__":
    main()
