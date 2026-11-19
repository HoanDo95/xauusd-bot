#!/usr/bin/env python3
"""Pre-armed limit OB retest screen (research only).

Execution model that can match a real limit fill: send the order at the
displacement close (arm alert), rest a limit at the broken level for 8 bars,
SL at the displacement extreme + 3 USD (known at arm time), TP1 1R / TP2 2R,
50/50, BE after TP1, 96-bar timeout. No rejection-close filter (the limit fills
on the touch).

This is the only model where the subscriber can actually get the measured
entry. It is tested because the post-close alert entry is not executable.

Usage:
  python3 scripts/research/scalp_arm_limit_ab.py \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_ARM_LIMIT_AB_2026-11-19.md
"""

import argparse
import importlib.util
import json
import math
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
TIMEOUT_BARS = 96
BUFFER_USD = 3.0


def load_module():
    path = Path(__file__).with_name("scalp_m15_standalone_ab.py")
    spec = importlib.util.spec_from_file_location("scalp_m15_standalone_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", default="rust-backend/data/backtest/XAUUSDm_M15.jsonl")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    base = load_module()
    bars = base.load_bars(args.cache)
    atr = base.atr14_series(bars)
    h1 = base.aggregate(bars, H1_MS)
    h4 = base.aggregate(bars, H4_MS)
    d1 = base.aggregate(bars, D1_MS)

    h4_bias = base.bias_series(h4["c"], base.ema_series(h4["c"], 50), base.ema_series(h4["c"], 200))
    d1_bias = base.bias_series(d1["c"], base.ema_series(d1["c"], 50), base.ema_series(d1["c"], 200))
    h4_close = [ts + H4_MS for ts in h4["ts"]]
    d1_close = [ts + D1_MS for ts in d1["ts"]]
    h1_rsi = base.rsi_series(h1["c"])
    h1_close = [ts + H1_MS for ts in h1["ts"]]

    start_time = d1_close[199] if len(d1_close) > 200 else 0
    import bisect

    start = bisect.bisect_left(bars["ts"], start_time)

    trades = []
    skipped = {"htf": 0, "rsi": 0, "no_fill": 0}
    j = max(start, 20)
    next_free = j
    while j < bars["n"] - 2:
        if j < next_free:
            j += 1
            continue
        found = base.displacement_at(bars, atr, j)
        if not found:
            j += 1
            continue
        direction, level = found
        signal_time = bars["ts"][j] + M15_MS
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
            skipped["htf"] += 1
            j += 1
            continue
        rsi_index = base.last_closed_index(h1_close, signal_time)
        rsi_value = h1_rsi[rsi_index] if rsi_index is not None else None
        if direction == "BUY" and rsi_value is not None and rsi_value < 30.0:
            skipped["rsi"] += 1
            j += 1
            continue

        stop_loss = (
            bars["h"][j] + BUFFER_USD if direction == "SELL" else bars["l"][j] - BUFFER_USD
        )
        fill_bar = None
        fill_price = None
        for k in range(j + 1, min(j + WAIT_BARS, bars["n"] - 2) + 1):
            if direction == "SELL":
                if bars["o"][k] >= level:
                    fill_bar, fill_price = k, bars["o"][k]
                    break
                if bars["h"][k] >= level:
                    fill_bar, fill_price = k, level
                    break
            else:
                if bars["o"][k] <= level:
                    fill_bar, fill_price = k, bars["o"][k]
                    break
                if bars["l"][k] <= level:
                    fill_bar, fill_price = k, level
                    break
        if fill_bar is None or abs(fill_price - stop_loss) <= 0:
            skipped["no_fill"] += 1
            j += 1
            continue

        result = base.simulate(bars, direction, fill_price, stop_loss, fill_bar, COST_USD)
        result1 = base.simulate(
            bars, direction, fill_price, stop_loss, fill_bar, COST_USD_STRESS
        )
        if result is None or result1 is None:
            skipped["no_fill"] += 1
            j += 1
            continue
        trades.append(
            {
                "time": signal_time,
                "year": datetime.fromtimestamp(signal_time / 1000, tz=timezone.utc).year,
                "direction": direction,
                "entry": fill_price,
                "stop_loss": stop_loss,
                "risk_usd": abs(fill_price - stop_loss),
                "r": result[0],
                "r1": result1[0],
                "bars": result[1],
            }
        )
        next_free = fill_bar + result[1] + 1
        j = next_free

    weeks = (bars["ts"][-1] - bars["ts"][0]) / WEEK_MS
    values = [trade["r"] for trade in trades]
    stress = [trade["r1"] for trade in trades]
    summary = base.stats(values)
    summary_stress = base.stats(stress)
    risks = [trade["risk_usd"] for trade in trades]
    fills = len(trades)
    signals = fills + skipped["no_fill"]

    lines = [
        "# Scalp pre-armed limit OB retest — research screen",
        "",
        "> Execution model: arm at displacement close, limit at the broken level,",
        "> 8-bar hold, SL at the displacement extreme + 3 USD. Research only.",
        "",
        f"- Data: **{bars['n']}** M15 bars",
        f"- Signals: **{signals}**, fills **{fills}** ({fills/max(1,signals)*100:.0f}%), "
        f"skipped htf={skipped['htf']} rsi={skipped['rsi']}",
        f"- Trades/week: **{len(trades)/weeks:.2f}**",
        f"- Mean risk: **{sum(risks)/len(risks):.2f} USD/oz**",
        "",
        "| Metric | Cost 0.50 | Cost 1.00 |",
        "|---|---:|---:|",
        f"| n | {summary['n']} | {summary_stress['n']} |",
        f"| Win% | {summary['win']} | {summary_stress['win']} |",
        f"| Avg R | {summary['avg']:+.3f} | {summary_stress['avg']:+.3f} |",
        f"| Total R | {summary['total']:+.1f} | {summary_stress['total']:+.1f} |",
        f"| PF | {summary['pf']:.2f} | {summary_stress['pf']:.2f} |",
        f"| MaxDD | {summary['dd']:.1f} | {summary_stress['dd']:.1f} |",
        "",
        "## Yearly (cost 0.50)",
        "",
        "| Year | n | Win% | Avg R | PF |",
        "|---|---:|---:|---:|---:|",
    ]
    for year in sorted({trade["year"] for trade in trades}):
        group = [trade["r"] for trade in trades if trade["year"] == year]
        stats = base.stats(group)
        lines.append(
            f"| {year} | {stats['n']} | {stats['win']} | {stats['avg']:+.3f} | {stats['pf']:.2f} |"
        )
    lines.extend(
        [
            "",
            "## Reading",
            "",
            "- This is the only model where the subscriber can get the measured entry",
            "  (the limit is resting before the touch).",
            "- If it is negative, the level-entry edge is not tradeable as an alert and",
            "  the scalp product needs a different entry rule.",
            "",
        ]
    )
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
