#!/usr/bin/env python3
"""Scalp OB bounce screen (research only).

Pre-registration:
docs/codexminimal/research/2026-11-19-scalp-ob-bounce-preregistration.md

Plan: M15 OB bounce, entry at rejection close, SL beyond OB + 3 USD,
TP1 1R / TP2 2R, 50/50, BE after TP1, 96-bar timeout. HTF H4/D1 filter,
BUY RSI H1 < 30 guard. One trade at a time, one alert per OB timestamp.

Usage:
  python3 scripts/research/scalp_ob_bounce_ab.py \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_OB_BOUNCE_AB_2026-11-19.md
"""

import argparse
import importlib.util
import math
import statistics
from pathlib import Path

M15_MS = 15 * 60 * 1000
H1_MS = 60 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
D1_MS = 24 * 60 * 60 * 1000
COST_USD = 0.5
COST_USD_STRESS = 1.0
TIMEOUT_BARS = 96
LOOKBACK_MAX = 50
LOOKBACK_MIN = 5
BUFFER_USD = 3.0


def load_relax_module():
    path = Path(__file__).with_name("scalp_relaxation_ab.py")
    spec = importlib.util.spec_from_file_location("scalp_relaxation_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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


def find_ob(candles, atrs, index, direction):
    start = max(15, index - LOOKBACK_MAX)
    for j in range(index - LOOKBACK_MIN, start - 1, -1):
        candle = candles[j]
        atr = atrs[j]
        if atr is None or atr <= 0:
            continue
        previous = candles[max(0, j - 10) : j]
        if len(previous) < 10:
            continue
        candle_range = candle["high"] - candle["low"]
        if direction == "SELL":
            displacement = (
                candle["close"] < candle["open"]
                and candle_range >= atr
                and candle["close"] < min(item["low"] for item in previous)
            )
        else:
            displacement = (
                candle["close"] > candle["open"]
                and candle_range >= atr
                and candle["close"] > max(item["high"] for item in previous)
            )
        if not displacement:
            continue
        for k in range(j - 1, max(-1, j - 6), -1):
            ob = candles[k]
            is_opposite = (
                ob["close"] > ob["open"] if direction == "SELL" else ob["close"] < ob["open"]
            )
            if not is_opposite:
                continue
            return (
                k,
                min(ob["open"], ob["close"]),
                max(ob["open"], ob["close"]),
            )
        return None
    return None


def bounce(candle, direction, zone_low, zone_high):
    if direction == "SELL":
        return (
            candle["high"] >= zone_low
            and candle["close"] < zone_low
            and candle["close"] < candle["open"]
        )
    return (
        candle["low"] <= zone_high
        and candle["close"] > zone_high
        and candle["close"] > candle["open"]
    )


def rsi_series(closes, period=14):
    if len(closes) <= period:
        return [None] * len(closes)
    gains = []
    losses = []
    for i in range(1, len(closes)):
        change = closes[i] - closes[i - 1]
        gains.append(max(change, 0.0))
        losses.append(max(-change, 0.0))
    avg_gain = sum(gains[:period]) / period
    avg_loss = sum(losses[:period]) / period
    out = [None] * len(closes)
    for i in range(period, len(closes)):
        if i > period:
            avg_gain = (avg_gain * (period - 1) + gains[i - 1]) / period
            avg_loss = (avg_loss * (period - 1) + losses[i - 1]) / period
        if avg_loss <= 0:
            out[i] = 100.0
        else:
            rs = avg_gain / avg_loss
            out[i] = 100.0 - 100.0 / (1.0 + rs)
    return out


def rsi_at(h1, rsi_values, timestamp_ms):
    index = None
    for i, candle in enumerate(h1):
        if candle["timestamp"] + H1_MS <= timestamp_ms:
            index = i
        else:
            break
    if index is None:
        return None
    return rsi_values[index]


def htf_ok(direction, h4_bias, d1_bias):
    if direction == "SELL":
        return h4_bias != "BULLISH" and d1_bias != "BULLISH"
    return h4_bias != "BEARISH" and d1_bias != "BEARISH"


def simulate(direction, entry, stop_loss, start_index, candles, cost_usd):
    risk = abs(entry - stop_loss)
    if risk <= 0 or start_index >= len(candles):
        return None
    if direction == "SELL":
        tp1 = entry - risk
        tp2 = entry - 2.0 * risk
    else:
        tp1 = entry + risk
        tp2 = entry + 2.0 * risk
    tp1_hit = False
    for offset in range(1, TIMEOUT_BARS + 1):
        index = start_index + offset
        if index >= len(candles):
            break
        candle = candles[index]
        if not tp1_hit:
            if direction == "SELL":
                if candle["high"] >= stop_loss:
                    return -1.0 - cost_usd / risk, offset
                if candle["low"] <= tp1:
                    tp1_hit = True
            else:
                if candle["low"] <= stop_loss:
                    return -1.0 - cost_usd / risk, offset
                if candle["high"] >= tp1:
                    tp1_hit = True
        else:
            if direction == "SELL" and candle["high"] >= entry:
                return 0.5 - cost_usd / risk, offset
            if direction == "BUY" and candle["low"] <= entry:
                return 0.5 - cost_usd / risk, offset
        if tp1_hit:
            if direction == "SELL" and candle["low"] <= tp2:
                return 1.5 - cost_usd / risk, offset
            if direction == "BUY" and candle["high"] >= tp2:
                return 1.5 - cost_usd / risk, offset
    index = min(start_index + TIMEOUT_BARS, len(candles) - 1)
    close = candles[index]["close"]
    close_r = (
        (entry - close) / risk if direction == "SELL" else (close - entry) / risk
    )
    if tp1_hit:
        return 0.5 + 0.5 * close_r - cost_usd / risk, TIMEOUT_BARS
    return close_r - cost_usd / risk, TIMEOUT_BARS


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
        "# Scalp OB Bounce — research screen",
        "",
        "> Historical in-sample screen. Pre-registration:",
        "> `docs/codexminimal/research/2026-11-19-scalp-ob-bounce-preregistration.md`.",
        "> No live changes.",
        "",
        f"- Trades: **{len(trades)}** / skipped {skipped}",
        f"- Trades/week: **{len(trades)/weeks:.2f}**",
        f"- Cost: 0.50 USD/oz primary; 1.00 USD/oz stress",
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
        f"| OB retest (baseline) | 1.68 | +0.426 | 2.37 | 8.3 |",
        f"| OB bounce | {len(trades)/weeks:.2f} | {base['avg']:+.3f} | {base['pf']:.2f} | {base['dd']:.1f} |",
        "",
        "## Reading",
        "",
        "- Candidate only if cost 1.0 avg R is positive and PF > 1.2.",
        "- This is not forward OOS; no promotion without a shadow period.",
        "- If frequency is still low, add another setup or a lower timeframe; do not",
        "  loosen the validated OB retest filters.",
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

    relax = load_relax_module()
    candles = relax.load_ab_module().load_candles(args.cache)
    atrs = atr14_series(candles)
    h1 = relax.aggregate_tf(candles, H1_MS)
    h4 = relax.aggregate_tf(candles, H4_MS)
    d1 = relax.aggregate_tf(candles, D1_MS)
    h1_closes = [candle["close"] for candle in h1]
    h1_rsi = rsi_series(h1_closes)
    h4_ema50 = relax.ema_series([candle["close"] for candle in h4], 50)
    h4_ema200 = relax.ema_series([candle["close"] for candle in h4], 200)
    d1_ema50 = relax.ema_series([candle["close"] for candle in d1], 50)
    d1_ema200 = relax.ema_series([candle["close"] for candle in d1], 200)

    trades = []
    used_obs = set()
    skipped = 0
    i = 20
    while i < len(candles) - 1:
        candle = candles[i]
        signal = None
        for direction in ("SELL", "BUY"):
            ob = find_ob(candles, atrs, i, direction)
            if not ob:
                continue
            ob_index, zone_low, zone_high = ob
            key = (ob_index, direction)
            if key in used_obs:
                continue
            if not bounce(candle, direction, zone_low, zone_high):
                continue
            signal_close = candle["timestamp"] + M15_MS
            h4_bias = relax.bias_at(h4, h4_ema50, h4_ema200, signal_close, H4_MS)
            d1_bias = relax.bias_at(d1, d1_ema50, d1_ema200, signal_close, D1_MS)
            if not htf_ok(direction, h4_bias, d1_bias):
                continue
            rsi = rsi_at(h1, h1_rsi, signal_close)
            if direction == "BUY" and rsi is not None and rsi < 30.0:
                continue
            entry = candle["close"]
            stop_loss = zone_high + BUFFER_USD if direction == "SELL" else zone_low - BUFFER_USD
            result = simulate(direction, entry, stop_loss, i, candles, COST_USD)
            result1 = simulate(direction, entry, stop_loss, i, candles, COST_USD_STRESS)
            if result is None or result1 is None:
                skipped += 1
                continue
            signal = {
                "ob_index": ob_index,
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
        used_obs.add((signal["ob_index"], signal["direction"]))
        trades.append(signal)
        # One trade at a time; next signal after the exit candle.
        i = max(i + 1, i + signal["bars"] + 1)

    weeks = (candles[-1]["timestamp"] - candles[0]["timestamp"]) / (7 * 24 * 60 * 60 * 1000)
    report = render(trades, weeks, skipped)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
