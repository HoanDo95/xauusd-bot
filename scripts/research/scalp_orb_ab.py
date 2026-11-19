#!/usr/bin/env python3
"""Scalp London opening range breakout (research only).

Pre-registration:
docs/codexminimal/research/2026-11-20-scalp-orb-preregistration.md

Asian range 00:00-07:00 UTC, first close outside the range in 07:00-12:00 UTC.
Entry at the close, SL at the range midpoint (primary) or the breakout extreme,
TP1 1R / TP2 2R, BE after TP1, 96-bar timeout. Executable close entry.

Usage:
  python3 scripts/research/scalp_orb_ab.py --tf M15 \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_ORB_AB_2026-11-20.md
"""

import argparse
import bisect
import importlib.util
import random
from datetime import datetime, timezone
from pathlib import Path

M15_MS = 15 * 60 * 1000
M5_MS = 5 * 60 * 1000
H1_MS = 60 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
D1_MS = 24 * 60 * 60 * 1000
DAY_MS = 24 * 60 * 60 * 1000
WEEK_MS = 7 * DAY_MS
ASIAN_END_MS = 7 * 60 * 60 * 1000
WINDOW_END_MS = 12 * 60 * 60 * 1000
COST_USD = 0.5
COST_USD_STRESS = 1.0
BUFFER_USD = 3.0


def load_module():
    path = Path(__file__).with_name("scalp_m15_standalone_ab.py")
    spec = importlib.util.spec_from_file_location("scalp_m15_standalone_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def weekly_bootstrap(trades, iterations=1000, seed=13):
    if not trades:
        return (0.0, 0.0)
    weeks = {}
    for trade in trades:
        weeks.setdefault(trade["ts"] // WEEK_MS, []).append(trade["r"])
    keys = list(weeks)
    rng = random.Random(seed)
    means = []
    for _ in range(iterations):
        values = []
        for _ in keys:
            values.extend(weeks[rng.choice(keys)])
        means.append(sum(values) / len(values))
    means.sort()
    return (round(means[int(0.025 * len(means))], 3), round(means[int(0.975 * len(means))], 3))


def run(bars, tf_ms, h4, d1, h1, base, sl_mode="mid", use_htf=False):
    h4_close = [ts + H4_MS for ts in h4["ts"]]
    d1_close = [ts + D1_MS for ts in d1["ts"]]
    h1_close = [ts + H1_MS for ts in h1["ts"]]
    h4_bias = base.bias_series(h4["c"], base.ema_series(h4["c"], 50), base.ema_series(h4["c"], 200))
    d1_bias = base.bias_series(d1["c"], base.ema_series(d1["c"], 50), base.ema_series(d1["c"], 200))
    h1_rsi = base.rsi_series(h1["c"])

    days = {}
    for index, ts in enumerate(bars["ts"]):
        day = ts // DAY_MS
        days.setdefault(day, []).append(index)

    trades = []
    next_free_ts = 0
    for day in sorted(days):
        window_start = day * DAY_MS + ASIAN_END_MS
        if window_start < next_free_ts:
            continue
        indices = days[day]
        asian = [
            index
            for index in indices
            if (bars["ts"][index] - day * DAY_MS) < ASIAN_END_MS
        ]
        breakout = [
            index
            for index in indices
            if ASIAN_END_MS <= (bars["ts"][index] - day * DAY_MS) < WINDOW_END_MS
        ]
        if len(asian) < 20 or len(breakout) < 4:
            continue
        range_high = max(bars["h"][index] for index in asian)
        range_low = min(bars["l"][index] for index in asian)
        if range_high <= range_low:
            continue
        midpoint = (range_high + range_low) / 2.0
        for index in breakout:
            close = bars["c"][index]
            if close > range_high:
                direction = "BUY"
            elif close < range_low:
                direction = "SELL"
            else:
                continue
            signal_time = bars["ts"][index] + tf_ms
            if use_htf:
                h4_index = base.last_closed_index(h4_close, signal_time)
                d1_index = base.last_closed_index(d1_close, signal_time)
                if h4_index is None or d1_index is None:
                    break
                htf_pass = (
                    h4_bias[h4_index] != "BULLISH" and d1_bias[d1_index] != "BULLISH"
                    if direction == "SELL"
                    else h4_bias[h4_index] != "BEARISH" and d1_bias[d1_index] != "BEARISH"
                )
                if not htf_pass:
                    break
            rsi_index = base.last_closed_index(h1_close, signal_time)
            rsi_value = h1_rsi[rsi_index] if rsi_index is not None else None
            if direction == "BUY" and rsi_value is not None and rsi_value < 30.0:
                break
            entry = close
            if sl_mode == "mid":
                stop_loss = midpoint
            else:
                stop_loss = (
                    bars["h"][index] + BUFFER_USD
                    if direction == "SELL"
                    else bars["l"][index] - BUFFER_USD
                )
            if abs(entry - stop_loss) <= 0:
                break
            result = base.simulate(bars, direction, entry, stop_loss, index, COST_USD)
            result1 = base.simulate(
                bars, direction, entry, stop_loss, index, COST_USD_STRESS
            )
            if result is None or result1 is None:
                break
            trades.append(
                {
                    "ts": bars["ts"][index],
                    "month": datetime.fromtimestamp(
                        bars["ts"][index] / 1000, tz=timezone.utc
                    ).strftime("%Y-%m"),
                    "direction": direction,
                    "entry": entry,
                    "stop_loss": stop_loss,
                    "risk_usd": abs(entry - stop_loss),
                    "r": result[0],
                    "r1": result1[0],
                    "bars": result[1],
                }
            )
            next_free_ts = bars["ts"][index] + result[1] * tf_ms
            break
    weeks = (bars["ts"][-1] - bars["ts"][0]) / WEEK_MS
    return trades, weeks


def row(name, trades, weeks, base, key="r"):
    summary = base.stats([trade[key] for trade in trades])
    return (
        f"| {name} | {summary['n']} | {summary['n']/weeks:.2f} | {summary['win']} | "
        f"{summary['avg']:+.3f} | {summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} |"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tf", choices=("M15", "M5"), default="M15")
    parser.add_argument("--cache", default="")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    base = load_module()
    tf_ms = M15_MS if args.tf == "M15" else M5_MS
    cache = args.cache or (
        "rust-backend/data/backtest/XAUUSDm_M15.jsonl"
        if args.tf == "M15"
        else "rust-backend/data/backtest/XAUUSDm_M5.json"
    )
    bars = base.load_bars(cache)
    h1 = base.aggregate(bars, H1_MS)
    h4 = base.aggregate(bars, H4_MS)
    d1 = base.aggregate(bars, D1_MS)

    primary, weeks = run(bars, tf_ms, h4, d1, h1, base, "mid", False)
    mid_htf, _ = run(bars, tf_ms, h4, d1, h1, base, "mid", True)
    extreme, _ = run(bars, tf_ms, h4, d1, h1, base, "extreme", False)

    primary_stats = base.stats([trade["r"] for trade in primary])
    primary_stress = base.stats([trade["r1"] for trade in primary])
    ci = weekly_bootstrap(primary)
    risks = [trade["risk_usd"] for trade in primary]

    lines = [
        f"# Scalp London ORB — {args.tf} screen",
        "",
        "> Pre-registration:",
        "> `docs/codexminimal/research/2026-11-20-scalp-orb-preregistration.md`.",
        "> Executable close entries only. No live changes.",
        "",
        f"- Data: **{bars['n']}** {args.tf} bars, {bars['symbol']} {bars['server']}",
        f"- Weeks: **{weeks:.1f}**",
        f"- Trades: **{len(primary)}**",
        f"- Mean risk: **{sum(risks)/len(risks):.2f} USD/oz**",
        "",
        "## Primary — midpoint SL, no HTF",
        "",
        "| Metric | Cost 0.50 | Cost 1.00 |",
        "|---|---:|---:|",
        f"| n | {primary_stats['n']} | {primary_stress['n']} |",
        f"| Trades/week | {primary_stats['n']/weeks:.2f} | {primary_stress['n']/weeks:.2f} |",
        f"| Win% | {primary_stats['win']} | {primary_stress['win']} |",
        f"| Avg R | {primary_stats['avg']:+.3f} | {primary_stress['avg']:+.3f} |",
        f"| Total R | {primary_stats['total']:+.1f} | {primary_stress['total']:+.1f} |",
        f"| PF | {primary_stats['pf']:.2f} | {primary_stress['pf']:.2f} |",
        f"| MaxDD | {primary_stats['dd']:.1f} | {primary_stress['dd']:.1f} |",
        f"| Weekly bootstrap 95% CI (cost 0.50) | [{ci[0]:+.3f}, {ci[1]:+.3f}] | — |",
        "",
        "## Secondary variants (cost 0.50)",
        "",
        "| Variant | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        row("Midpoint SL + HTF", mid_htf, weeks, base),
        row("Breakout-extreme SL, no HTF", extreme, weeks, base),
        "",
        "## Secondary variants (cost 1.00)",
        "",
        "| Variant | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        row("Midpoint SL + HTF", mid_htf, weeks, base, "r1"),
        row("Breakout-extreme SL, no HTF", extreme, weeks, base, "r1"),
        "",
        "## Monthly — primary (cost 0.50)",
        "",
        "| Month | n | Win% | Avg R | PF |",
        "|---|---:|---:|---:|---:|",
    ]
    for month in sorted({trade["month"] for trade in primary}):
        group = [trade["r"] for trade in primary if trade["month"] == month]
        stats = base.stats(group)
        lines.append(
            f"| {month} | {stats['n']} | {stats['win']} | {stats['avg']:+.3f} | {stats['pf']:.2f} |"
        )

    buys = [trade for trade in primary if trade["direction"] == "BUY"]
    sells = [trade for trade in primary if trade["direction"] == "SELL"]
    lines.extend(
        [
            "",
            "## Side split — primary (cost 0.50)",
            "",
            "| Side | n | Win% | Avg R | PF |",
            "|---|---:|---:|---:|---:|",
        ]
    )
    for name, group in (("BUY", buys), ("SELL", sells)):
        stats = base.stats([trade["r"] for trade in group])
        lines.append(
            f"| {name} | {stats['n']} | {stats['win']} | {stats['avg']:+.3f} | {stats['pf']:.2f} |"
        )

    lines.extend(
        [
            "",
            "## Reading",
            "",
            "- Candidate only if cost 1.0 avg R > 0, PF > 1.2, n >= 200, >= 2.5/week.",
            "- M5 robustness must not flip negative.",
            "- No tuning after results; forward shadow remains the gate.",
            "",
        ]
    )
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
