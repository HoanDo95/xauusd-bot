#!/usr/bin/env python3
"""Scalp momentum continuation screen (research only).

Pre-registration:
docs/codexminimal/research/2026-11-19-scalp-momentum-preregistration.md

Displacement + BOS, then a same-direction continuation candle. Entry at the
continuation close, SL beyond the displacement candle + 3 USD, TP1 1R / TP2 2R,
50/50, BE after TP1, 96-bar timeout. HTF H4/D1 + BUY RSI H1 guard.

Usage:
  python3 scripts/research/scalp_momentum_ab.py \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_MOMENTUM_AB_2026-11-19.md
"""

import argparse
import importlib.util
import math
from pathlib import Path

M15_MS = 15 * 60 * 1000
COST_USD = 0.5
COST_USD_STRESS = 1.0
BUFFER_USD = 3.0


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


def render(trades, weeks, skipped):
    values = [trade["r"] for trade in trades]
    stress = [trade["r1"] for trade in trades]
    base = stats(values)
    stress_stats = stats(stress)
    lines = [
        "# Scalp Momentum Continuation — research screen",
        "",
        "> Historical in-sample screen. Pre-registration:",
        "> `docs/codexminimal/research/2026-11-19-scalp-momentum-preregistration.md`.",
        "> No live changes.",
        "",
        f"- Trades: **{len(trades)}** / skipped {skipped}",
        f"- Trades/week: **{len(trades)/weeks:.2f}**",
        "- Cost: 0.50 USD/oz primary; 1.00 USD/oz stress",
        "",
        "| Metric | Cost 0.50 | Cost 1.00 |",
        "|---|---:|---:|",
        f"| n | {base['n']} | {stress_stats['n']} |",
        f"| Win% | {base['win']} | {stress_stats['win']} |",
        f"| Avg R | {base['avg']:+.3f} | {stress_stats['avg']:+.3f} |",
        f"| Total R | {base['total']:+.1f} | {stress_stats['total']:+.1f} |",
        f"| PF | {base['pf']:.2f} | {stress_stats['pf']:.2f} |",
        f"| MaxDD | {base['dd']:.1f} | {stress_stats['dd']:.1f} |",
        "",
        "## Comparison",
        "",
        "| Setup | Alerts/week | Avg R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|",
        "| OB retest (baseline) | 1.68 | +0.426 | 2.37 | 8.3 |",
        f"| Momentum continuation | {len(trades)/weeks:.2f} | {base['avg']:+.3f} | {base['pf']:.2f} | {base['dd']:.1f} |",
        "",
        "## Reading",
        "",
        "- Proceed only if cost 1.0 avg R > 0 and PF > 1.2.",
        "- This is historical in-sample; no live promotion without forward shadow.",
        "- If negative, do not add to scalp; try M5 or another instrument next.",
        "",
    ]
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--cache",
        default="rust-backend/data/backtest/XAUUSDm_M15.jsonl",
    )
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    relax = load_module("scalp_relaxation_ab", "scalp_relaxation_ab.py")
    bounce = load_module("scalp_ob_bounce_ab", "scalp_ob_bounce_ab.py")
    candles = relax.load_ab_module().load_candles(args.cache)
    atrs = bounce.atr14_series(candles)
    h1 = relax.aggregate_tf(candles, bounce.H1_MS)
    h4 = relax.aggregate_tf(candles, bounce.H4_MS)
    d1 = relax.aggregate_tf(candles, bounce.D1_MS)
    h1_rsi = bounce.rsi_series([candle["close"] for candle in h1])
    h4_ema50 = relax.ema_series([candle["close"] for candle in h4], 50)
    h4_ema200 = relax.ema_series([candle["close"] for candle in h4], 200)
    d1_ema50 = relax.ema_series([candle["close"] for candle in d1], 50)
    d1_ema200 = relax.ema_series([candle["close"] for candle in d1], 200)

    trades = []
    used = set()
    skipped = 0
    i = 20
    while i < len(candles) - 1:
        j = i - 1
        atr = atrs[j]
        previous = candles[max(0, j - 10) : j]
        signal = None
        if atr is not None and atr > 0 and len(previous) >= 10:
            displacement = candles[j]
            continuation = candles[i]
            for direction in ("SELL", "BUY"):
                if (j, direction) in used:
                    continue
                if direction == "SELL":
                    is_displacement = (
                        displacement["close"] < displacement["open"]
                        and (displacement["high"] - displacement["low"]) >= atr
                        and displacement["close"]
                        < min(item["low"] for item in previous)
                    )
                    is_continuation = (
                        continuation["close"] < continuation["open"]
                        and continuation["close"] < displacement["close"]
                    )
                else:
                    is_displacement = (
                        displacement["close"] > displacement["open"]
                        and (displacement["high"] - displacement["low"]) >= atr
                        and displacement["close"]
                        > max(item["high"] for item in previous)
                    )
                    is_continuation = (
                        continuation["close"] > continuation["open"]
                        and continuation["close"] > displacement["close"]
                    )
                if not is_displacement or not is_continuation:
                    continue
                signal_close = continuation["timestamp"] + M15_MS
                h4_bias = relax.bias_at(h4, h4_ema50, h4_ema200, signal_close, bounce.H4_MS)
                d1_bias = relax.bias_at(d1, d1_ema50, d1_ema200, signal_close, bounce.D1_MS)
                if not bounce.htf_ok(direction, h4_bias, d1_bias):
                    continue
                rsi = bounce.rsi_at(h1, h1_rsi, signal_close)
                if direction == "BUY" and rsi is not None and rsi < 30.0:
                    continue
                entry = continuation["close"]
                stop_loss = (
                    displacement["high"] + BUFFER_USD
                    if direction == "SELL"
                    else displacement["low"] - BUFFER_USD
                )
                result = bounce.simulate(
                    direction, entry, stop_loss, i, candles, COST_USD
                )
                result1 = bounce.simulate(
                    direction, entry, stop_loss, i, candles, COST_USD_STRESS
                )
                if result is None or result1 is None:
                    skipped += 1
                    continue
                signal = {
                    "j": j,
                    "direction": direction,
                    "entry": entry,
                    "stop_loss": stop_loss,
                    "r": result[0],
                    "r1": result1[0],
                    "bars": result[1],
                }
                break
        if signal is None:
            i += 1
            continue
        used.add((signal["j"], signal["direction"]))
        trades.append(signal)
        i = max(i + 1, i + signal["bars"] + 1)

    weeks = (candles[-1]["timestamp"] - candles[0]["timestamp"]) / (
        7 * 24 * 60 * 60 * 1000
    )
    report = render(trades, weeks, skipped)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
