#!/usr/bin/env python3
"""ICT/OB entry-refinement A/B on the retest variant.

Pre-registration: docs/codexminimal/research/2026-11-19-ict-ob-preregistration.md
Research only. Does not change live signals.

Features (exactly one definition each):
  sweep      prior 20-bar swing sweep + close back inside, >= 0.1 ATR
  pd         SELL premium / BUY discount in the last 20 closed H1 bars
  ob_near    nearest body OB distance <= 1.0 ATR after displacement + BOS

Usage:
  python3 scripts/research/ict_ob_ab.py \
    --trades rust-backend/data/backtest/exness_retest_variant6.json \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/ICT_OB_AB_2026-11-19.md
"""

import argparse
import importlib.util
import json
import math
import random
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path

M15_MS = 15 * 60 * 1000
H1_MS = 60 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
VN_OFFSET = timedelta(hours=7)
BASE_COST_R = 0.02
MDE_R = 0.30
ALPHA = 0.05


def load_ab_module():
    path = Path(__file__).with_name("entry_sl_ab.py")
    spec = importlib.util.spec_from_file_location("entry_sl_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def parse_ts(value):
    if not value:
        return None
    text = value.replace("Z", "+00:00")
    if "." in text:
        head, rest = text.split(".", 1)
        fraction, sep, tail = rest.partition("+")
        if not sep:
            fraction, sep, tail = rest.partition("-")
        text = f"{head}.{fraction[:6]}{sep}{tail}"
    return datetime.fromisoformat(text)


def parse_ms(value):
    ts = parse_ts(value)
    if not ts:
        return None
    return int(ts.timestamp() * 1000)


def vn_hour(value):
    ts = parse_ts(value)
    if not ts:
        return None
    return (ts + VN_OFFSET).hour


def aggregate_tf(candles, tf_ms):
    out = []
    for candle in candles:
        bucket = candle["timestamp"] - candle["timestamp"] % tf_ms
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


def atr14_series(candles):
    atrs = []
    for index in range(len(candles)):
        if index < 14:
            atrs.append(None)
            continue
        values = []
        for i in range(index - 13, index + 1):
            candle = candles[i]
            prev = candles[i - 1]["close"] if i > 0 else candle["close"]
            tr = max(
                candle["high"] - candle["low"],
                abs(candle["high"] - prev),
                abs(candle["low"] - prev),
            )
            values.append(tr)
        atrs.append(sum(values) / len(values))
    return atrs


def h4_atr_at(h4, atrs, timestamp_ms):
    index = None
    for i, candle in enumerate(h4):
        if candle["timestamp"] + H4_MS <= timestamp_ms:
            index = i
        else:
            break
    if index is None:
        return None
    return atrs[index]


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


def week_key(date_str):
    ts = parse_ts(date_str)
    if not ts:
        return "unknown"
    year, week, _ = ts.isocalendar()
    return f"{year}-W{week:02d}"


def block_bootstrap(rows, iterations=3000):
    blocks = defaultdict(list)
    for date_str, value in rows:
        blocks[week_key(date_str)].append(value)
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


def permutation_p(rows_a, rows_b, iterations=3000):
    values_a = [value for _, value in rows_a]
    values_b = [value for _, value in rows_b]
    if len(values_a) < 10 or len(values_b) < 10:
        return None
    observed = abs(
        sum(values_a) / len(values_a) - sum(values_b) / len(values_b)
    )
    values = values_a + values_b
    random.seed(7)
    count = 0
    for _ in range(iterations):
        random.shuffle(values)
        a = values[: len(values_a)]
        b = values[len(values_a) :]
        diff = abs(sum(a) / len(a) - sum(b) / len(b))
        if diff >= observed:
            count += 1
    return count / iterations


def holm(pvalues):
    items = [(name, p) for name, p in pvalues.items() if p is not None]
    items.sort(key=lambda item: item[1])
    total = len(items)
    out = {}
    previous = 0.0
    for index, (name, p) in enumerate(items):
        adjusted = min(1.0, max(previous, p * (total - index)))
        out[name] = adjusted
        previous = adjusted
    return out


def mde_80(std, n_a, n_b):
    if n_a <= 0 or n_b <= 0:
        return None
    se = std * math.sqrt(1.0 / n_a + 1.0 / n_b)
    return round((1.96 + 0.84) * se, 3)


def pearson(left, right):
    if len(left) < 3 or len(left) != len(right):
        return None
    mean_l = sum(left) / len(left)
    mean_r = sum(right) / len(right)
    cov = sum((l - mean_l) * (r - mean_r) for l, r in zip(left, right))
    var_l = sum((l - mean_l) ** 2 for l in left)
    var_r = sum((r - mean_r) ** 2 for r in right)
    if var_l <= 0 or var_r <= 0:
        return None
    return cov / math.sqrt(var_l * var_r)


def rank(values):
    order = sorted(range(len(values)), key=lambda i: values[i])
    ranks = [0.0] * len(values)
    index = 0
    while index < len(order):
        end = index
        while end + 1 < len(order) and values[order[end + 1]] == values[order[index]]:
            end += 1
        average = (index + end) / 2.0 + 1.0
        for i in range(index, end + 1):
            ranks[order[i]] = average
        index = end + 1
    return ranks


def spearman(left, right):
    return pearson(rank(left), rank(right))


def sweep_feature(candles, index, direction, atr):
    if index < 20 or atr is None or atr <= 0:
        return None
    prior = candles[index - 20 : index]
    prior_high = max(candle["high"] for candle in prior)
    prior_low = min(candle["low"] for candle in prior)
    candle = candles[index]
    if direction == "SELL":
        swept = (
            candle["high"] > prior_high
            and candle["close"] < prior_high
            and (candle["high"] - prior_high) >= 0.1 * atr
        )
    else:
        swept = (
            candle["low"] < prior_low
            and candle["close"] > prior_low
            and (prior_low - candle["low"]) >= 0.1 * atr
        )
    return 1 if swept else 0


def pd_feature(h1, timestamp_ms, entry, direction):
    closed = [
        candle
        for candle in h1
        if candle["timestamp"] + H1_MS <= timestamp_ms
    ][-20:]
    if len(closed) < 20:
        return None
    high = max(candle["high"] for candle in closed)
    low = min(candle["low"] for candle in closed)
    if high <= low:
        return None
    position = (entry - low) / (high - low)
    if direction == "SELL":
        return 1 if position >= 0.5 else 0
    return 1 if position <= 0.5 else 0


def ob_distance(candles, index, direction, atr, entry):
    if atr is None or atr <= 0 or index < 20:
        return None
    start = max(15, index - 50)
    for j in range(index, start - 1, -1):
        candle = candles[j]
        previous = candles[max(0, j - 10) : j]
        if len(previous) < 10:
            continue
        candle_range = candle["high"] - candle["low"]
        if direction == "SELL":
            displacement = (
                candle["close"] < candle["open"]
                and candle_range >= 1.0 * atr
                and candle["close"] < min(item["low"] for item in previous)
            )
        else:
            displacement = (
                candle["close"] > candle["open"]
                and candle_range >= 1.0 * atr
                and candle["close"] > max(item["high"] for item in previous)
            )
        if not displacement:
            continue
        for k in range(j - 1, max(-1, j - 6), -1):
            ob = candles[k]
            if direction == "SELL":
                is_opposite = ob["close"] > ob["open"]
            else:
                is_opposite = ob["close"] < ob["open"]
            if not is_opposite:
                continue
            zone_low = min(ob["open"], ob["close"])
            zone_high = max(ob["open"], ob["close"])
            if direction == "SELL":
                return max(0.0, (entry - zone_high) / atr)
            return max(0.0, (zone_low - entry) / atr)
        return None
    return None


def wick_body(candle):
    body = abs(candle["close"] - candle["open"])
    upper = candle["high"] - max(candle["open"], candle["close"])
    lower = min(candle["open"], candle["close"]) - candle["low"]
    return max(upper, lower, 0.0) / max(body, 0.01)


def load_trades(path):
    data = json.load(open(path))
    return data["variants"]["quantum_v3_htf_retest"]["trades"]


def build_rows(trades, candles, ab, h1, h4, h4_atrs, m15_atrs, ema50):
    rows = []
    skipped = 0
    for trade in trades:
        fill_ms = parse_ms(trade.get("fill_time"))
        if fill_ms is None:
            skipped += 1
            continue
        index = next(
            (
                i
                for i, candle in enumerate(candles)
                if candle["timestamp"] == fill_ms - M15_MS
            ),
            None,
        )
        if index is None:
            skipped += 1
            continue
        direction = trade.get("direction")
        entry = trade.get("entry")
        risk = trade.get("risk")
        if direction not in ("BUY", "SELL") or not entry or not risk or risk <= 0:
            skipped += 1
            continue
        atr = m15_atrs[index]
        h4_atr = h4_atr_at(h4, h4_atrs, fill_ms)
        if h4_atr is None or h4_atr <= 0:
            skipped += 1
            continue
        net = trade["r_multiple"] - (0.5 / risk - BASE_COST_R)
        net1 = trade["r_multiple"] - (1.0 / risk - BASE_COST_R)
        ema = ema50[index] if index < len(ema50) else None
        rows.append(
            {
                "date": trade["signal_time"][:10],
                "trade": trade,
                "index": index,
                "direction": direction,
                "entry": entry,
                "risk": risk,
                "r": net,
                "r1": net1,
                "sweep": sweep_feature(candles, index, direction, atr),
                "pd": pd_feature(h1, fill_ms, entry, direction),
                "ob": ob_distance(candles, index, direction, atr, entry),
                "distance_ema_atr": abs(entry - ema) / h4_atr if ema else None,
                "rsi_h1": trade.get("rsi_h1"),
                "atr_percentile": ab.percentile_at(h4, h4_atrs, fill_ms),
                "sl_distance_atr": risk / h4_atr,
                "wick_body_ratio": wick_body(candles[index]),
            }
        )
    return rows, skipped


def rows_for(rows, feature):
    selected = []
    for row in rows:
        if feature == "sweep" and row["sweep"] == 1:
            selected.append((row["date"], row["r"]))
        elif feature == "pd" and row["pd"] == 1:
            selected.append((row["date"], row["r"]))
        elif feature == "ob" and row["ob"] is not None and row["ob"] <= 1.0:
            selected.append((row["date"], row["r"]))
    return selected


def rows_for_cost1(rows, feature):
    selected = []
    for row in rows:
        if feature == "sweep" and row["sweep"] == 1:
            selected.append((row["date"], row["r1"]))
        elif feature == "pd" and row["pd"] == 1:
            selected.append((row["date"], row["r1"]))
        elif feature == "ob" and row["ob"] is not None and row["ob"] <= 1.0:
            selected.append((row["date"], row["r1"]))
    return selected


def complement(rows, feature):
    selected = []
    for row in rows:
        if feature == "sweep" and row["sweep"] == 0:
            selected.append((row["date"], row["r"]))
        elif feature == "pd" and row["pd"] == 0:
            selected.append((row["date"], row["r"]))
        elif feature == "ob" and (row["ob"] is None or row["ob"] > 1.0):
            selected.append((row["date"], row["r"]))
    return selected


def fmt_ci(ci):
    return f"{ci[0]:+.3f} … {ci[1]:+.3f}" if ci else "n/a"


def render(rows, skipped):
    baseline = [(row["date"], row["r"]) for row in rows]
    baseline1 = [(row["date"], row["r1"]) for row in rows]
    baseline_values = [value for _, value in baseline]
    std = 0.0
    if len(baseline_values) > 1:
        mean = sum(baseline_values) / len(baseline_values)
        std = math.sqrt(
            sum((value - mean) ** 2 for value in baseline_values)
            / (len(baseline_values) - 1)
        )

    lines = [
        "# ICT/OB A/B — 2026-11-19",
        "",
        "> **Caveat:** historical in-sample screen on the retest variant, not true",
        "> forward OOS. No live changes. Pre-registration:",
        "> `docs/codexminimal/research/2026-11-19-ict-ob-preregistration.md`.",
        "",
        f"- Retest trades: **{len(rows)}** used / {skipped} skipped (missing candle)",
        f"- Baseline cost: **0.50 USD/oz**; stress: **1.00 USD/oz**",
        f"- Baseline std: **{std:.3f}R**; MDE(80%) ≈ **{mde_80(std, 150, max(1, len(rows)-150)):.3f}R** for a 150-trade split",
        "",
        "## Feature counts",
        "",
        "| Feature | Present | Missing |",
        "|---|---:|---:|",
        f"| sweep | {sum(1 for row in rows if row['sweep'] == 1)} | {sum(1 for row in rows if row['sweep'] is None)} |",
        f"| premium/discount | {sum(1 for row in rows if row['pd'] == 1)} | {sum(1 for row in rows if row['pd'] is None)} |",
        f"| OB <= 1.0 ATR | {sum(1 for row in rows if row['ob'] is not None and row['ob'] <= 1.0)} | {sum(1 for row in rows if row['ob'] is None)} |",
        "",
        "## Correlation with existing features (Spearman)",
        "",
        "| New feature | distance_ema_atr | rsi_h1 | atr_percentile | sl_distance_atr | wick_body_ratio |",
        "|---|---:|---:|---:|---:|---:|",
    ]

    def corr_row(name, values):
        cells = []
        for existing in [
            "distance_ema_atr",
            "rsi_h1",
            "atr_percentile",
            "sl_distance_atr",
            "wick_body_ratio",
        ]:
            pairs = [
                (values[i], row[existing])
                for i, row in enumerate(rows)
                if values[i] is not None and row[existing] is not None
            ]
            if len(pairs) < 10:
                cells.append("n/a")
            else:
                left = [pair[0] for pair in pairs]
                right = [pair[1] for pair in pairs]
                value = spearman(left, right)
                cells.append(f"{value:+.2f}" if value is not None else "n/a")
        lines.append(f"| {name} | " + " | ".join(cells) + " |")

    corr_row("sweep", [row["sweep"] for row in rows])
    corr_row("premium/discount", [row["pd"] for row in rows])
    corr_row("OB distance", [row["ob"] for row in rows])

    lines.extend(
        [
            "",
            "## A/B — cost 0.50 USD/oz",
            "",
            "| Group | n | Win% | Avg R | Total R | PF | MaxDD | Bootstrap 95% CI |",
            "|---|---:|---:|---:|---:|---:|---:|---|",
        ]
    )

    def table_row(name, selected):
        summary = stats([value for _, value in selected])
        lines.append(
            f"| {name} | {summary['n']} | {summary['win']} | {summary['avg']:+.3f} | "
            f"{summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} | "
            f"{fmt_ci(block_bootstrap(selected))} |"
        )

    table_row("baseline retest", baseline)
    pvalues = {}
    for feature, label in [
        ("sweep", "sweep"),
        ("pd", "premium/discount"),
        ("ob", "OB <= 1.0 ATR"),
    ]:
        selected = rows_for(rows, feature)
        excluded = complement(rows, feature)
        table_row(label, selected)
        pvalue = permutation_p(selected, excluded)
        pvalues[label] = pvalue

    adjusted = holm(pvalues)
    lines.extend(
        [
            "",
            "## Primary tests",
            "",
            "| Feature | n filtered | Avg R filtered | Avg R excluded | Effect | Permutation p | Holm p | MDE(80%) | Verdict |",
            "|---|---:|---:|---:|---:|---:|---:|---:|---|",
        ]
    )
    for feature, label in [
        ("sweep", "sweep"),
        ("pd", "premium/discount"),
        ("ob", "OB <= 1.0 ATR"),
    ]:
        selected = rows_for(rows, feature)
        excluded = complement(rows, feature)
        selected_stats = stats([value for _, value in selected])
        excluded_stats = stats([value for _, value in excluded])
        mde = mde_80(std, len(selected), len(excluded))
        pvalue = pvalues.get(label)
        holm_p = adjusted.get(label)
        effect = selected_stats["avg"] - excluded_stats["avg"]
        if len(selected) < 30 or len(excluded) < 30:
            verdict = "insufficient n"
        elif pvalue is None:
            verdict = "insufficient n"
        elif holm_p is not None and holm_p < ALPHA and effect >= MDE_R:
            verdict = "pass"
        elif holm_p is not None and holm_p < ALPHA and effect <= -MDE_R:
            verdict = "reject (negative)"
        elif abs(effect) < MDE_R:
            verdict = "inconclusive (<MDE)"
        else:
            verdict = "inconclusive"
        lines.append(
            f"| {label} | {selected_stats['n']} | {selected_stats['avg']:+.3f} | "
            f"{excluded_stats['avg']:+.3f} | {effect:+.3f} | "
            f"{pvalue if pvalue is not None else 'n/a'} | "
            f"{holm_p if holm_p is not None else 'n/a'} | {mde if mde is not None else 'n/a'} | {verdict} |"
        )

    lines.extend(
        [
            "",
            "## Cost stress — 1.00 USD/oz",
            "",
            "| Group | n | Win% | Avg R | Total R | PF | MaxDD |",
            "|---|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for name, selected in [
        ("baseline retest", baseline1),
        ("sweep", rows_for_cost1(rows, "sweep")),
        ("premium/discount", rows_for_cost1(rows, "pd")),
        ("OB <= 1.0 ATR", rows_for_cost1(rows, "ob")),
    ]:
        summary = stats([value for _, value in selected])
        lines.append(
            f"| {name} | {summary['n']} | {summary['win']} | {summary['avg']:+.3f} | "
            f"{summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} |"
        )

    lines.extend(
        [
            "",
            "## Reading the result",
            "",
            "- A feature only passes if Holm p < 0.05, |effect| >= 0.30R, and the",
            "  filtered sample has enough trades.",
            "- `inconclusive (<MDE)` means the data cannot distinguish the filter from",
            "  noise; do not promote or reject on that result alone.",
            "- Correlation |rho| > 0.70 means the new feature is largely redundant with",
            "  an existing feature; treat it as the same experiment.",
            "- No combinations were tested in this primary round.",
            "- No live changes. If a feature passes, the next step is a separate shadow",
            "  profile and forward test, not a live promotion.",
            "",
        ]
    )
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--trades",
        default="rust-backend/data/backtest/exness_retest_variant6.json",
    )
    parser.add_argument(
        "--cache",
        default="rust-backend/data/backtest/XAUUSDm_M15.jsonl",
    )
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    ab = load_ab_module()
    candles = ab.load_candles(args.cache)
    h1 = aggregate_tf(candles, H1_MS)
    h4 = ab.aggregate_h4(candles)
    h4_atrs = ab.h4_atr_series(h4)
    # EMA series is needed per-index, not the final value.
    ema50_series = []
    alpha = 2.0 / (50.0 + 1.0)
    current = candles[0]["close"] if candles else 0.0
    for candle in candles:
        current = alpha * candle["close"] + (1.0 - alpha) * current
        ema50_series.append(current)

    trades = load_trades(args.trades)
    m15_atrs = atr14_series(candles)
    rows, skipped = build_rows(
        trades, candles, ab, h1, h4, h4_atrs, m15_atrs, ema50_series
    )
    report = render(rows, skipped)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
