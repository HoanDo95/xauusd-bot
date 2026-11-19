#!/usr/bin/env python3
"""Scalp intrabar M1 entry screen (research only).

Pre-registration:
docs/codexminimal/research/2026-11-20-scalp-intrabar-m1-preregistration.md

Tests an executable entry inside the M15 retest candle using M1 closes:
displacement + first touch of the broken level + M1 rejection close. SL is
anchored to the high/low seen so far + 3 USD. Outcomes are simulated on M1.

Usage:
  python3 scripts/research/scalp_intrabar_m1_ab.py \
    --m15 rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --m1 rust-backend/data/backtest/XAUUSDm_M1_300k.json \
    --out ../docs/SCALP_INTRABAR_M1_AB_2026-11-20.md
"""

import argparse
import bisect
import importlib.util
import json
import math
import random
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

M15_MS = 15 * 60 * 1000
H1_MS = 60 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
D1_MS = 24 * 60 * 60 * 1000
WEEK_MS = 7 * 24 * 60 * 60 * 1000

COST_USD = 0.5
COST_USD_STRESS = 1.0
WAIT_BARS = 8
TIMEOUT_MS = 96 * M15_MS
BUFFER_USD = 3.0
OB_MAX_ATR = 1.0
BOUNCE_ATR = 0.10


def load_module():
    path = Path(__file__).with_name("scalp_m15_standalone_ab.py")
    spec = importlib.util.spec_from_file_location("scalp_m15_standalone_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_m15(path):
    rows = [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]
    return {
        "ts": [row["timestamp"] for row in rows],
        "o": [row["open"] for row in rows],
        "h": [row["high"] for row in rows],
        "l": [row["low"] for row in rows],
        "c": [row["close"] for row in rows],
        "v": [row.get("volume", 0.0) for row in rows],
        "n": len(rows),
    }


def load_m1(path):
    payload = json.loads(Path(path).read_text())
    bars = payload["bars"]
    scale = 1000 if bars and bars[0]["t"] < 10**12 else 1
    return {
        "ts": [bar["t"] * scale for bar in bars],
        "o": [bar["o"] for bar in bars],
        "h": [bar["h"] for bar in bars],
        "l": [bar["l"] for bar in bars],
        "c": [bar["c"] for bar in bars],
        "n": len(bars),
    }


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


def simulate_m1(m1, direction, entry, stop_loss, entry_idx, entry_ts, cost_usd):
    risk = abs(entry - stop_loss)
    if not risk or risk <= 0:
        return None
    if direction == "SELL":
        tp1 = entry - risk
        tp2 = entry - 2.0 * risk
    else:
        tp1 = entry + risk
        tp2 = entry + 2.0 * risk
    end_ts = entry_ts + TIMEOUT_MS
    tp1_hit = False
    last_index = entry_idx
    for index in range(entry_idx + 1, m1["n"]):
        ts = m1["ts"][index]
        if ts > end_ts:
            break
        last_index = index
        high, low = m1["h"][index], m1["l"][index]
        if not tp1_hit:
            if direction == "SELL":
                if high >= stop_loss:
                    return -1.0 - cost_usd / risk, index, "SL"
                if low <= tp1:
                    tp1_hit = True
            else:
                if low <= stop_loss:
                    return -1.0 - cost_usd / risk, index, "SL"
                if high >= tp1:
                    tp1_hit = True
        else:
            if direction == "SELL" and high >= entry:
                return 0.5 - cost_usd / risk, index, "BE"
            if direction == "BUY" and low <= entry:
                return 0.5 - cost_usd / risk, index, "BE"
        if tp1_hit:
            if direction == "SELL" and low <= tp2:
                return 1.5 - cost_usd / risk, index, "TP2"
            if direction == "BUY" and high >= tp2:
                return 1.5 - cost_usd / risk, index, "TP2"
    close = m1["c"][last_index]
    if direction == "SELL":
        close_r = (entry - close) / risk
    else:
        close_r = (close - entry) / risk
    if tp1_hit:
        return 0.5 + 0.5 * close_r - cost_usd / risk, last_index, "TIMEOUT_TP1"
    return close_r - cost_usd / risk, last_index, "TIMEOUT"


def ob_exists(m15, j, direction):
    for k in range(j - 1, max(-1, j - 6), -1):
        if direction == "SELL":
            if m15["c"][k] > m15["o"][k]:
                return True
        elif m15["c"][k] < m15["o"][k]:
            return True
    return False


def weekly_bootstrap(trades, iterations=1000, seed=7):
    if not trades:
        return (0.0, 0.0)
    weeks = {}
    for trade in trades:
        week = trade["entry_ts"] // WEEK_MS
        weeks.setdefault(week, []).append(trade["r"])
    keys = list(weeks)
    rng = random.Random(seed)
    means = []
    for _ in range(iterations):
        values = []
        for _ in keys:
            key = rng.choice(keys)
            values.extend(weeks[key])
        means.append(sum(values) / len(values))
    means.sort()
    return (round(means[int(0.025 * len(means))], 3), round(means[int(0.975 * len(means))], 3))


def run_variant(m15, m1, atr, h4, d1, h1, base, variant="v0", sl_mode="watch"):
    h4_close = [ts + H4_MS for ts in h4["ts"]]
    d1_close = [ts + D1_MS for ts in d1["ts"]]
    h1_close = [ts + H1_MS for ts in h1["ts"]]
    h4_bias = base.bias_series(h4["c"], base.ema_series(h4["c"], 50), base.ema_series(h4["c"], 200))
    d1_bias = base.bias_series(d1["c"], base.ema_series(d1["c"], 50), base.ema_series(d1["c"], 200))
    h1_rsi = base.rsi_series(h1["c"])

    m1_start, m1_end = m1["ts"][0], m1["ts"][-1]
    start = bisect.bisect_left(m15["ts"], m1_start)
    counters = Counter()
    trades = []
    j = max(start, 20)
    next_free_ts = m15["ts"][j]
    while j < m15["n"] - 2:
        if m15["ts"][j] < next_free_ts:
            j += 1
            continue
        if m15["ts"][j] + (WAIT_BARS + 1) * M15_MS > m1_end:
            break
        found = base.displacement_at(m15, atr, j)
        if not found:
            j += 1
            continue
        direction, level = found
        if not ob_exists(m15, j, direction):
            j += 1
            continue
        atr_j = atr[j]
        if atr_j is None or atr_j <= 0:
            j += 1
            continue
        signal_time = m15["ts"][j] + M15_MS
        h4_index = base.last_closed_index(h4_close, signal_time)
        d1_index = base.last_closed_index(d1_close, signal_time)
        if h4_index is None or d1_index is None:
            j += 1
            continue
        htf_pass = (
            h4_bias[h4_index] != "BULLISH" and d1_bias[d1_index] != "BULLISH"
            if direction == "SELL"
            else h4_bias[h4_index] != "BEARISH" and d1_bias[d1_index] != "BEARISH"
        )
        if not htf_pass:
            counters["htf_blocked"] += 1
            j += 1
            continue
        rsi_index = base.last_closed_index(h1_close, signal_time)
        rsi_value = h1_rsi[rsi_index] if rsi_index is not None else None
        if direction == "BUY" and rsi_value is not None and rsi_value < 30.0:
            counters["rsi_blocked"] += 1
            j += 1
            continue

        counters["armed"] += 1
        original_stop = (
            m15["h"][j] + BUFFER_USD if direction == "SELL" else m15["l"][j] - BUFFER_USD
        )
        original_risk = abs(level - original_stop)
        if original_risk <= 0:
            j += 1
            continue

        high_so_far = float("-inf")
        low_so_far = float("inf")
        entry = None
        entry_idx = None
        entry_m15 = None
        gap = False
        cancelled = False
        for k in range(j + 1, j + WAIT_BARS + 1):
            window_start = m15["ts"][k]
            start_idx = bisect.bisect_left(m1["ts"], window_start)
            end_idx = bisect.bisect_left(m1["ts"], window_start + M15_MS)
            if end_idx - start_idx < 10:
                gap = True
                break
            if sl_mode == "retest":
                high_so_far = float("-inf")
                low_so_far = float("inf")
            for idx in range(start_idx, end_idx):
                high, low, close = m1["h"][idx], m1["l"][idx], m1["c"][idx]
                invalidated = (
                    low < level - original_risk
                    if direction == "SELL"
                    else high > level + original_risk
                )
                if invalidated:
                    counters["invalidated"] += 1
                    cancelled = True
                    break
                high_so_far = max(high_so_far, high)
                low_so_far = min(low_so_far, low)
                if direction == "SELL":
                    touched = high >= level
                    rejected = close < level and close < m1["o"][idx]
                    if variant == "v1":
                        rejected = close <= level - BOUNCE_ATR * atr_j and close < m1["o"][idx]
                else:
                    touched = low <= level
                    rejected = close > level and close > m1["o"][idx]
                    if variant == "v1":
                        rejected = close >= level + BOUNCE_ATR * atr_j and close > m1["o"][idx]
                if not touched or not rejected:
                    continue
                if variant == "v2":
                    if idx + 1 >= m1["n"]:
                        cancelled = True
                        break
                    entry_idx = idx + 1
                    entry = m1["c"][entry_idx]
                    entry_invalid = (
                        m1["l"][entry_idx] < level - original_risk
                        if direction == "SELL"
                        else m1["h"][entry_idx] > level + original_risk
                    )
                    if entry_invalid:
                        counters["invalidated"] += 1
                        cancelled = True
                        break
                    high_so_far = max(high_so_far, m1["h"][entry_idx])
                    low_so_far = min(low_so_far, m1["l"][entry_idx])
                else:
                    entry_idx = idx
                    entry = close
                entry_m15 = k
                break
            if entry is not None or gap or cancelled:
                break
        if gap:
            counters["data_gap"] += 1
            j += 1
            continue
        if cancelled:
            j += 1
            continue
        if entry is None:
            counters["expired"] += 1
            j += 1
            continue

        stop_loss = (
            max(level, high_so_far) + BUFFER_USD
            if direction == "SELL"
            else min(level, low_so_far) - BUFFER_USD
        )
        if abs(entry - stop_loss) <= 0:
            j += 1
            continue
        entry_ts = m1["ts"][entry_idx]
        result = simulate_m1(m1, direction, entry, stop_loss, entry_idx, entry_ts, COST_USD)
        result1 = simulate_m1(
            m1, direction, entry, stop_loss, entry_idx, entry_ts, COST_USD_STRESS
        )
        if result is None or result1 is None:
            j += 1
            continue
        counters["entries"] += 1
        trades.append(
            {
                "entry_ts": entry_ts,
                "entry_idx": entry_idx,
                "m15_index": entry_m15,
                "displacement_index": j,
                "level": level,
                "month": datetime.fromtimestamp(entry_ts / 1000, tz=timezone.utc).strftime("%Y-%m"),
                "direction": direction,
                "entry": entry,
                "stop_loss": stop_loss,
                "risk_usd": abs(entry - stop_loss),
                "r": result[0],
                "r1": result1[0],
                "outcome": result[2],
            }
        )
        exit_ts = m1["ts"][result[1]]
        next_free_ts = exit_ts + M15_MS
        j = entry_m15 + 1
    return trades, counters

def table_row(name, trades, weeks, key="r"):
    summary = stats([trade[key] for trade in trades])
    return (
        f"| {name} | {summary['n']} | {summary['n']/weeks:.2f} | {summary['win']} | "
        f"{summary['avg']:+.3f} | {summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} |"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--m15", default="rust-backend/data/backtest/XAUUSDm_M15.jsonl")
    parser.add_argument("--m1", default="rust-backend/data/backtest/XAUUSDm_M1_300k.json")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    base = load_module()
    m15 = load_m15(args.m15)
    m1 = load_m1(args.m1)
    atr = base.atr14_series(m15)
    h1 = base.aggregate(m15, H1_MS)
    h4 = base.aggregate(m15, H4_MS)
    d1 = base.aggregate(m15, D1_MS)

    m1_weeks = (m1["ts"][-1] - m1["ts"][0]) / WEEK_MS

    v0, counters0 = run_variant(m15, m1, atr, h4, d1, h1, base, "v0")
    v1, counters1 = run_variant(m15, m1, atr, h4, d1, h1, base, "v1")
    v2, counters2 = run_variant(m15, m1, atr, h4, d1, h1, base, "v2")
    v3, counters3 = run_variant(m15, m1, atr, h4, d1, h1, base, "v0", sl_mode="retest")

    # Same-period M15 baselines.
    m15_start, m15_end = m1["ts"][0], m15["ts"][-1]
    close_all, _ = base.run(m15, atr, h4, d1, h1, entry_mode="close")
    level_all, _ = base.run(m15, atr, h4, d1, h1, entry_mode="level")
    close_base = [t for t in close_all if m15_start <= t["time"] <= m15_end]
    level_base = [t for t in level_all if m15_start <= t["time"] <= m15_end]

    summary = stats([trade["r"] for trade in v0])
    summary_stress = stats([trade["r1"] for trade in v0])
    ci = weekly_bootstrap(v0)
    risks = [trade["risk_usd"] for trade in v0]

    lines = [
        "# Scalp Intrabar M1 Entry — research screen",
        "",
        "> Pre-registration:",
        "> `docs/codexminimal/research/2026-11-20-scalp-intrabar-m1-preregistration.md`.",
        "> Executable M1 entries only. No live changes.",
        "",
        f"- M15 bars: **{m15['n']}**; M1 bars: **{m1['n']}**",
        f"- Test window: 2026-01-16 → 2026-11-17 ({m1_weeks:.1f} weeks)",
        f"- V0 armed **{counters0['armed']}**, entries **{counters0['entries']}**, "
        f"invalidated **{counters0['invalidated']}**, expired **{counters0['expired']}**, "
        f"gaps **{counters0['data_gap']}**",
        f"- Mean risk: **{sum(risks)/len(risks):.2f} USD/oz**",
        "",
        "## V0 — first M1 rejection at the level",
        "",
        "| Metric | Cost 0.50 | Cost 1.00 |",
        "|---|---:|---:|",
        f"| n | {summary['n']} | {summary_stress['n']} |",
        f"| Win% | {summary['win']} | {summary_stress['win']} |",
        f"| Avg R | {summary['avg']:+.3f} | {summary_stress['avg']:+.3f} |",
        f"| Total R | {summary['total']:+.1f} | {summary_stress['total']:+.1f} |",
        f"| PF | {summary['pf']:.2f} | {summary_stress['pf']:.2f} |",
        f"| MaxDD | {summary['dd']:.1f} | {summary_stress['dd']:.1f} |",
        f"| Weekly bootstrap 95% CI (cost 0.50) | [{ci[0]:+.3f}, {ci[1]:+.3f}] | — |",
        "",
        "## Variants and same-period baselines",
        "",
        "| Setup | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        table_row("V0 M1 rejection (primary)", v0, m1_weeks),
        table_row("V1 rejection >=0.10 ATR", v1, m1_weeks),
        table_row("V2 +1 M1 bar delay", v2, m1_weeks),
        table_row("V3 retest-candle SL (exploratory)", v3, m1_weeks),
        table_row("B1 M15 close entry", close_base, m1_weeks),
        table_row("B2 M15 level entry (not fillable)", level_base, m1_weeks),
        "",
        "## V0 cost stress 1.00",
        "",
        "| Setup | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        table_row("V0 M1 rejection (primary)", v0, m1_weeks, "r1"),
        table_row("V1 rejection >=0.10 ATR", v1, m1_weeks, "r1"),
        table_row("V2 +1 M1 bar delay", v2, m1_weeks, "r1"),
        table_row("V3 retest-candle SL (exploratory)", v3, m1_weeks, "r1"),
        table_row("B1 M15 close entry", close_base, m1_weeks, "r1"),
        table_row("B2 M15 level entry (not fillable)", level_base, m1_weeks, "r1"),
        "",
        "## V0 monthly (cost 0.50)",
        "",
        "| Month | n | Win% | Avg R | PF |",
        "|---|---:|---:|---:|---:|",
    ]
    for month in sorted({trade["month"] for trade in v0}):
        group = [trade["r"] for trade in v0 if trade["month"] == month]
        month_stats = stats(group)
        lines.append(
            f"| {month} | {month_stats['n']} | {month_stats['win']} | "
            f"{month_stats['avg']:+.3f} | {month_stats['pf']:.2f} |"
        )

    buys = [trade for trade in v0 if trade["direction"] == "BUY"]
    sells = [trade for trade in v0 if trade["direction"] == "SELL"]
    outcome_counts = Counter(trade["outcome"] for trade in v0)
    lines.extend(
        [
            "",
            "## V0 outcomes (cost 0.50)",
            "",
            "| Outcome | n |",
            "|---|---:|",
            *[f"| {name} | {count} |" for name, count in outcome_counts.most_common()],
            "",
            f"- Break-even cost (linear from the two cost levels): "
            f"**{0.5 - summary['avg'] / ((summary_stress['avg'] - summary['avg']) / 0.5):.2f} USD/oz**",
            "",
            "## V0 side split (cost 0.50)",
            "",
            "| Side | n | Win% | Avg R | PF |",
            "|---|---:|---:|---:|---:|",
        ]
    )
    for name, group in (("BUY", buys), ("SELL", sells)):
        side = stats([trade["r"] for trade in group])
        lines.append(
            f"| {name} | {side['n']} | {side['win']} | {side['avg']:+.3f} | {side['pf']:.2f} |"
        )

    lines.extend(
        [
            "",
            "## Reading",
            "",
            "- Candidate only if cost 1.0 avg R > 0, PF > 1.2, n >= 100, >= 2.5/week.",
            "- This is 2026 only (M1 coverage); a pass is a forward-shadow candidate,",
            "  not a live promotion.",
            "- If V0 fails while B2 (level, not fillable) is strong, the OB retest idea",
            "  is closed for the scalp product.",
            "",
        ]
    )
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
