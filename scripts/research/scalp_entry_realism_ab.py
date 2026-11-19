#!/usr/bin/env python3
"""Execution-realism check for the scalp retest alerts (research only).

The alert fires at the retest candle close with entry = broken level. A
subscriber enters at market (the close), not at the level, and the structural
SL is fixed. This script measures the difference between the published
(level-entry) and the executable (close-entry) R for the current M15
engine+OB stream.

No live change. Not a promotion test.
"""

import argparse
import importlib.util
import math
from pathlib import Path

M15_MS = 15 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
D1_MS = 24 * 60 * 60 * 1000
COST_USD = 0.5
COST_USD_STRESS = 1.0
TIMEOUT_BARS = 96


def load_module(name, filename):
    path = Path(__file__).with_name(filename)
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--triggers",
        default="rust-backend/data/backtest/exness_raw_triggers_v3.jsonl",
    )
    parser.add_argument(
        "--cache",
        default="rust-backend/data/backtest/XAUUSDm_M15.jsonl",
    )
    parser.add_argument("--out", default="")
    parser.add_argument("--limit-hold-bars", type=int, default=8)
    args = parser.parse_args()

    relax = load_module("scalp_relaxation_ab", "scalp_relaxation_ab.py")
    ab = relax.load_ab_module()
    candles = ab.load_candles(args.cache)
    triggers = ab.load_triggers(args.triggers)
    h4 = relax.aggregate_tf(candles, H4_MS)
    d1 = relax.aggregate_tf(candles, D1_MS)
    h4_fast = relax.ema_series([c["close"] for c in h4], 50)
    h4_slow = relax.ema_series([c["close"] for c in h4], 200)
    d1_fast = relax.ema_series([c["close"] for c in d1], 50)
    d1_slow = relax.ema_series([c["close"] for c in d1], 200)

    index_by_ts = {candle["timestamp"]: i for i, candle in enumerate(candles)}
    rows = []
    for trigger in triggers:
        if not trigger.get("retest_ok"):
            continue
        if trigger.get("scenario_id") == "bullish_bounce":
            continue
        if (trigger.get("confidence") or 0) < 60:
            continue
        if not relax.macro_ok(trigger.get("macro_alignment")):
            continue
        direction = trigger.get("direction")
        entry = trigger.get("retest_entry")
        stop_loss = trigger.get("retest_stop_loss")
        retest_time = trigger.get("retest_time")
        if direction not in ("BUY", "SELL") or not entry or not stop_loss or not retest_time:
            continue
        retest_ms = relax.parse_ms(retest_time)
        index = index_by_ts.get(retest_ms - M15_MS)
        if index is None:
            continue
        h4_bias = relax.bias_at(h4, h4_fast, h4_slow, retest_ms, H4_MS)
        d1_bias = relax.bias_at(d1, d1_fast, d1_slow, retest_ms, D1_MS)
        if not relax.htf_ok(direction, h4_bias, d1_bias):
            continue
        if not relax.rsi_ok(direction, trigger.get("rsi_h1")):
            continue
        rows.append(
            {
                "index": index,
                "direction": direction,
                "level": entry,
                "stop_loss": stop_loss,
                "close": candles[index]["close"],
                "atr": None,
            }
        )

    # ATR14 at each retest candle for scale context.
    atr = [None] * len(candles)
    for i in range(14, len(candles)):
        total = 0.0
        for k in range(i - 13, i + 1):
            prev = candles[k - 1]["close"] if k > 0 else candles[k]["close"]
            total += max(
                candles[k]["high"] - candles[k]["low"],
                abs(candles[k]["high"] - prev),
                abs(candles[k]["low"] - prev),
            )
        atr[i] = total / 14.0

    level_rows = []
    close_rows = []
    limit_rows = []
    slip_usd = []
    slip_atr = []
    for row in rows:
        i = row["index"]
        result = relax.simulate_scalp(
            row["direction"], row["level"], row["stop_loss"], i, candles, COST_USD
        )
        result1 = relax.simulate_scalp(
            row["direction"], row["level"], row["stop_loss"], i, candles, COST_USD_STRESS
        )
        if result is None or result1 is None:
            continue
        # Market entry at the retest candle close, same structural SL.
        close_result = relax.simulate_scalp(
            row["direction"], row["close"], row["stop_loss"], i, candles, COST_USD
        )
        close_result1 = relax.simulate_scalp(
            row["direction"], row["close"], row["stop_loss"], i, candles, COST_USD_STRESS
        )
        if close_result is None or close_result1 is None:
            continue
        level_rows.append((result, result1))
        close_rows.append((close_result, close_result1))

        # Executable model B: limit at the broken level, placed after the retest
        # close, valid for N bars. Fill at the level (or the gap open).
        fill_index = None
        fill_price = None
        for k in range(i + 1, min(i + args.limit_hold_bars, len(candles) - 1) + 1):
            if row["direction"] == "SELL":
                if candles[k]["open"] >= row["level"]:
                    fill_index, fill_price = k, candles[k]["open"]
                    break
                if candles[k]["high"] >= row["level"]:
                    fill_index, fill_price = k, row["level"]
                    break
            else:
                if candles[k]["open"] <= row["level"]:
                    fill_index, fill_price = k, candles[k]["open"]
                    break
                if candles[k]["low"] <= row["level"]:
                    fill_index, fill_price = k, row["level"]
                    break
        if fill_index is not None:
            risk = abs(fill_price - row["stop_loss"])
            if risk > 0:
                limit_result = relax.simulate_scalp(
                    row["direction"], fill_price, row["stop_loss"], fill_index, candles, COST_USD
                )
                limit_result1 = relax.simulate_scalp(
                    row["direction"], fill_price, row["stop_loss"], fill_index, candles, COST_USD_STRESS
                )
                if limit_result is not None and limit_result1 is not None:
                    limit_rows.append((limit_result, limit_result1))
        gap = abs(row["level"] - row["close"])
        slip_usd.append(gap)
        if atr[i]:
            slip_atr.append(gap / atr[i])

    def summarize(pairs, index):
        return stats([pair[index] for pair in pairs])

    published = summarize(level_rows, 0)
    published_stress = summarize(level_rows, 1)
    market = summarize(close_rows, 0)
    market_stress = summarize(close_rows, 1)
    limit = summarize(limit_rows, 0)
    limit_stress = summarize(limit_rows, 1)

    lines = [
        "# Scalp alert execution realism — 2026-11-19",
        "",
        "> Research only. Measures published (level) vs executable (close) entry",
        "> for the current M15 engine+OB stream. No live change.",
        "",
        f"- Rows: **{len(close_rows)}**; limit fills **{len(limit_rows)}** "
        f"({len(limit_rows)/max(1,len(close_rows))*100:.0f}% of signals)",
        f"- Mean |level − close|: **{sum(slip_usd)/len(slip_usd):.2f} USD/oz** "
        f"(**{sum(slip_atr)/len(slip_atr):.2f} ATR**)",
        "",
        "| Execution | Cost | n | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---|---:|---:|---:|---:|---:|---:|",
        f"| Published (entry = level) | 0.50 | {published['n']} | {published['win']} | "
        f"{published['avg']:+.3f} | {published['total']:+.1f} | {published['pf']:.2f} | {published['dd']:.1f} |",
        f"| Published (entry = level) | 1.00 | {published_stress['n']} | {published_stress['win']} | "
        f"{published_stress['avg']:+.3f} | {published_stress['total']:+.1f} | {published_stress['pf']:.2f} | {published_stress['dd']:.1f} |",
        f"| Executable (entry = close) | 0.50 | {market['n']} | {market['win']} | "
        f"{market['avg']:+.3f} | {market['total']:+.1f} | {market['pf']:.2f} | {market['dd']:.1f} |",
        f"| Executable (entry = close) | 1.00 | {market_stress['n']} | {market_stress['win']} | "
        f"{market_stress['avg']:+.3f} | {market_stress['total']:+.1f} | {market_stress['pf']:.2f} | {market_stress['dd']:.1f} |",
        f"| Limit at level, {args.limit_hold_bars}-bar hold | 0.50 | {limit['n']} | {limit['win']} | "
        f"{limit['avg']:+.3f} | {limit['total']:+.1f} | {limit['pf']:.2f} | {limit['dd']:.1f} |",
        f"| Limit at level, {args.limit_hold_bars}-bar hold | 1.00 | {limit_stress['n']} | {limit_stress['win']} | "
        f"{limit_stress['avg']:+.3f} | {limit_stress['total']:+.1f} | {limit_stress['pf']:.2f} | {limit_stress['dd']:.1f} |",
        "",
        "## Reading",
        "",
        "- The alert fires at the retest candle close; a subscriber cannot fill at the",
        "  broken level unless price returns there.",
        "- If the close-entry row is much weaker, the published R is not executable and",
        "  the alert needs a different order model (limit at the level, or a new entry",
        "  rule) before it can be sold as a live signal.",
        "",
    ]
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
