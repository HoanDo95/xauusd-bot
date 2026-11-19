#!/usr/bin/env python3
"""Scalp standalone M15 OB retest screen (research only).

Pre-registration:
docs/codexminimal/research/2026-11-19-scalp-m15-standalone-preregistration.md

Self-contained OB retest on M15, no dependency on the main scenario engine:
displacement >= 1.0 ATR breaking the prior 10-bar extreme, wait up to 8 bars
for a rejection at the broken level, OB body distance <= 1.0 ATR, entry at the
broken level, SL beyond the retest candle + 3 USD, TP1 1R / TP2 2R, 50/50,
BE after TP1, 96-bar timeout. HTF H4/D1 filter, BUY RSI H1 < 30 guard.

Usage:
  python3 scripts/research/scalp_m15_standalone_ab.py \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_M15_STANDALONE_AB_2026-11-19.md
"""

import argparse
import bisect
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
OB_MAX_ATR = 1.0
TP1_R = 1.0
TP2_R = 2.0


def load_bars(path):
    text = Path(path).read_text()
    if path.endswith(".jsonl"):
        bars = [json.loads(line) for line in text.splitlines() if line.strip()]
        return {
            "ts": [bar["timestamp"] for bar in bars],
            "o": [bar["open"] for bar in bars],
            "h": [bar["high"] for bar in bars],
            "l": [bar["low"] for bar in bars],
            "c": [bar["close"] for bar in bars],
            "v": [bar.get("volume", 0.0) for bar in bars],
            "n": len(bars),
            "symbol": "XAUUSDm",
            "server": "Exness M15 dump",
        }
    payload = json.loads(text)
    bars = payload["bars"]
    # MT5 exports unix seconds; older research dumps use milliseconds.
    scale = 1000 if bars and bars[0]["t"] < 10**12 else 1
    return {
        "ts": [bar["t"] * scale for bar in bars],
        "o": [bar["o"] for bar in bars],
        "h": [bar["h"] for bar in bars],
        "l": [bar["l"] for bar in bars],
        "c": [bar["c"] for bar in bars],
        "v": [bar.get("v", 0) for bar in bars],
        "n": len(bars),
        "symbol": payload.get("symbol", ""),
        "server": payload.get("server", ""),
    }


def atr14_series(bars):
    n = bars["n"]
    h, l, c = bars["h"], bars["l"], bars["c"]
    tr = [0.0] * n
    for i in range(1, n):
        tr[i] = max(h[i] - l[i], abs(h[i] - c[i - 1]), abs(l[i] - c[i - 1]))
    atr = [None] * n
    if n <= 14:
        return atr
    window = sum(tr[1:15])
    atr[14] = window / 14.0
    for i in range(15, n):
        window += tr[i] - tr[i - 14]
        atr[i] = window / 14.0
    return atr


def aggregate(bars, tf_ms):
    out = {"ts": [], "o": [], "h": [], "l": [], "c": []}
    ts, o, h, l, c = bars["ts"], bars["o"], bars["h"], bars["l"], bars["c"]
    for i in range(len(ts)):
        bucket = ts[i] - ts[i] % tf_ms
        if not out["ts"] or out["ts"][-1] != bucket:
            out["ts"].append(bucket)
            out["o"].append(o[i])
            out["h"].append(h[i])
            out["l"].append(l[i])
            out["c"].append(c[i])
        else:
            out["h"][-1] = max(out["h"][-1], h[i])
            out["l"][-1] = min(out["l"][-1], l[i])
            out["c"][-1] = c[i]
    return out


def ema_series(values, period):
    if not values:
        return []
    alpha = 2.0 / (period + 1.0)
    current = values[0]
    out = [current]
    for value in values[1:]:
        current = alpha * value + (1.0 - alpha) * current
        out.append(current)
    return out


def bias_series(closes, ema_fast, ema_slow):
    out = []
    for close, fast, slow in zip(closes, ema_fast, ema_slow):
        if close > fast > slow:
            out.append("BULLISH")
        elif close < fast < slow:
            out.append("BEARISH")
        else:
            out.append("NEUTRAL")
    return out


def rsi_series(values, period=14):
    n = len(values)
    out = [None] * n
    if n <= period:
        return out
    gains = [0.0] * n
    losses = [0.0] * n
    for i in range(1, n):
        change = values[i] - values[i - 1]
        gains[i] = max(change, 0.0)
        losses[i] = max(-change, 0.0)
    avg_gain = sum(gains[1 : period + 1]) / period
    avg_loss = sum(losses[1 : period + 1]) / period
    for i in range(period, n):
        if i > period:
            avg_gain = (avg_gain * (period - 1) + gains[i]) / period
            avg_loss = (avg_loss * (period - 1) + losses[i]) / period
        if avg_loss <= 0:
            out[i] = 100.0
        else:
            rs = avg_gain / avg_loss
            out[i] = 100.0 - 100.0 / (1.0 + rs)
    return out


def last_closed_index(close_times, signal_time):
    index = bisect.bisect_right(close_times, signal_time) - 1
    return index if index >= 0 else None


def displacement_at(bars, atr, j):
    if atr[j] is None or atr[j] <= 0:
        return None
    window_low = min(bars["l"][j - 10 : j])
    window_high = max(bars["h"][j - 10 : j])
    candle_range = bars["h"][j] - bars["l"][j]
    if candle_range < atr[j]:
        return None
    if bars["c"][j] < bars["o"][j] and bars["c"][j] < window_low:
        return ("SELL", window_low)
    if bars["c"][j] > bars["o"][j] and bars["c"][j] > window_high:
        return ("BUY", window_high)
    return None


def ob_distance(bars, atr, j, direction, level, index):
    for k in range(j - 1, max(-1, j - 6), -1):
        ob = bars
        is_opposite = (
            ob["c"][k] > ob["o"][k]
            if direction == "SELL"
            else ob["c"][k] < ob["o"][k]
        )
        if not is_opposite:
            continue
        zone_low = min(ob["o"][k], ob["c"][k])
        zone_high = max(ob["o"][k], ob["c"][k])
        atr_value = atr[index]
        if atr_value is None or atr_value <= 0:
            return None
        if direction == "SELL":
            return max(0.0, (level - zone_high) / atr_value)
        return max(0.0, (zone_low - level) / atr_value)
    return None


def retest_signal(bars, i, direction, level):
    if direction == "SELL":
        return (
            bars["h"][i] >= level
            and bars["c"][i] < level
            and bars["c"][i] < bars["o"][i]
        )
    return (
        bars["l"][i] <= level
        and bars["c"][i] > level
        and bars["c"][i] > bars["o"][i]
    )


def simulate(bars, direction, entry, stop_loss, start_index, cost_usd):
    risk = abs(entry - stop_loss)
    if risk <= 0:
        return None
    n = bars["n"]
    h, l, c = bars["h"], bars["l"], bars["c"]
    if direction == "SELL":
        tp1 = entry - TP1_R * risk
        tp2 = entry - TP2_R * risk
    else:
        tp1 = entry + TP1_R * risk
        tp2 = entry + TP2_R * risk
    tp1_hit = False
    for offset in range(1, TIMEOUT_BARS + 1):
        index = start_index + offset
        if index >= n:
            break
        if not tp1_hit:
            if direction == "SELL":
                if h[index] >= stop_loss:
                    return -1.0 - cost_usd / risk, offset
                if l[index] <= tp1:
                    tp1_hit = True
            else:
                if l[index] <= stop_loss:
                    return -1.0 - cost_usd / risk, offset
                if h[index] >= tp1:
                    tp1_hit = True
        else:
            if direction == "SELL" and h[index] >= entry:
                return 0.5 - cost_usd / risk, offset
            if direction == "BUY" and l[index] <= entry:
                return 0.5 - cost_usd / risk, offset
        if tp1_hit:
            if direction == "SELL" and l[index] <= tp2:
                return 1.5 - cost_usd / risk, offset
            if direction == "BUY" and h[index] >= tp2:
                return 1.5 - cost_usd / risk, offset
    index = min(start_index + TIMEOUT_BARS, n - 1)
    close = c[index]
    if direction == "SELL":
        close_r = (entry - close) / risk
    else:
        close_r = (close - entry) / risk
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


def run(bars, atr, h4, d1, h1, entry_mode="level"):
    h4_fast = ema_series(h4["c"], 50)
    h4_slow = ema_series(h4["c"], 200)
    d1_fast = ema_series(d1["c"], 50)
    d1_slow = ema_series(d1["c"], 200)
    h4_bias = bias_series(h4["c"], h4_fast, h4_slow)
    d1_bias = bias_series(d1["c"], d1_fast, d1_slow)
    h4_close = [ts + H4_MS for ts in h4["ts"]]
    d1_close = [ts + D1_MS for ts in d1["ts"]]
    h1_rsi = rsi_series(h1["c"])
    h1_close = [ts + H1_MS for ts in h1["ts"]]

    # Require 200 closed H4/D1 bars before the first signal.
    start_time = d1_close[199] if len(d1_close) > 200 else 0
    start = bisect.bisect_left(bars["ts"], start_time)

    trades = []
    skipped_filter = {"htf": 0, "rsi": 0, "ob": 0}
    j = max(start, 20)
    next_free = j
    while j < bars["n"] - 2:
        if j < next_free:
            j += 1
            continue
        found = displacement_at(bars, atr, j)
        if not found:
            j += 1
            continue
        direction, level = found
        original_stop = (
            bars["h"][j] + BUFFER_USD if direction == "SELL" else bars["l"][j] - BUFFER_USD
        )
        original_risk = abs(level - original_stop)
        if original_risk <= 0:
            j += 1
            continue

        taken = False
        for i in range(j + 1, min(j + WAIT_BARS, bars["n"] - 2) + 1):
            invalidated = (
                bars["l"][i] < level - original_risk
                if direction == "SELL"
                else bars["h"][i] > level + original_risk
            )
            if invalidated:
                break
            if not retest_signal(bars, i, direction, level):
                continue

            signal_time = bars["ts"][i] + M15_MS
            h4_index = last_closed_index(h4_close, signal_time)
            d1_index = last_closed_index(d1_close, signal_time)
            if h4_index is None or d1_index is None:
                j = i + 1
                break
            htf_pass = (
                h4_bias[h4_index] != "BULLISH" and d1_bias[d1_index] != "BULLISH"
                if direction == "SELL"
                else h4_bias[h4_index] != "BEARISH" and d1_bias[d1_index] != "BEARISH"
            )
            if not htf_pass:
                skipped_filter["htf"] += 1
                j = i + 1
                break
            rsi_index = last_closed_index(h1_close, signal_time)
            rsi_value = h1_rsi[rsi_index] if rsi_index is not None else None
            if direction == "BUY" and rsi_value is not None and rsi_value < 30.0:
                skipped_filter["rsi"] += 1
                j = i + 1
                break
            distance = ob_distance(bars, atr, j, direction, level, i)
            if distance is None:
                skipped_filter["ob"] += 1
                j = i + 1
                break
            if distance > OB_MAX_ATR:
                skipped_filter["ob"] += 1
                j = i + 1
                break

            stop_loss = (
                max(bars["h"][i], level) + BUFFER_USD
                if direction == "SELL"
                else min(bars["l"][i], level) - BUFFER_USD
            )
            entry_bar = i
            if entry_mode == "level":
                entry = level
            elif entry_mode == "close":
                entry = bars["c"][i]
            elif entry_mode == "close_atr":
                entry = bars["c"][i]
                atr_entry = atr[i]
                if atr_entry is None or atr_entry <= 0:
                    j = i + 1
                    break
                stop_loss = (
                    entry + atr_entry if direction == "SELL" else entry - atr_entry
                )
            elif entry_mode == "confirm":
                confirm_bar = None
                for k in range(i + 1, min(i + 3, bars["n"] - 2) + 1):
                    if direction == "SELL" and bars["c"][k] < bars["l"][i]:
                        confirm_bar = k
                        break
                    if direction == "BUY" and bars["c"][k] > bars["h"][i]:
                        confirm_bar = k
                        break
                if confirm_bar is None:
                    j = i + 1
                    break
                entry_bar = confirm_bar
                entry = bars["c"][confirm_bar]
            else:
                entry = level
            result = simulate(bars, direction, entry, stop_loss, entry_bar, COST_USD)
            result1 = simulate(bars, direction, entry, stop_loss, entry_bar, COST_USD_STRESS)
            if result is None or result1 is None:
                j = i + 1
                break
            trades.append(
                {
                    "time": signal_time,
                    "year": datetime.fromtimestamp(signal_time / 1000, tz=timezone.utc).year,
                    "index": i,
                    "level": level,
                    "direction": direction,
                    "entry": entry,
                    "stop_loss": stop_loss,
                    "risk_usd": abs(entry - stop_loss),
                    "r": result[0],
                    "r1": result1[0],
                    "bars": result[1],
                    "entry_bar": entry_bar,
                    "ob_distance": round(distance, 3),
                }
            )
            next_free = entry_bar + result[1] + 1
            taken = True
            j = next_free
            break
        if not taken:
            j += 1
    return trades, skipped_filter


def year_table(trades, use_stress=False):
    key = "r1" if use_stress else "r"
    years = sorted({trade["year"] for trade in trades})
    lines = [
        "| Year | n | Win% | Avg R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for year in years:
        values = [trade[key] for trade in trades if trade["year"] == year]
        summary = stats(values)
        lines.append(
            f"| {year} | {summary['n']} | {summary['win']} | {summary['avg']:+.3f} | "
            f"{summary['pf']:.2f} | {summary['dd']:.1f} |"
        )
    return lines


def render(bars, trades, weeks, skipped, entry_mode="level"):
    values = [trade["r"] for trade in trades]
    stress = [trade["r1"] for trade in trades]
    base = stats(values)
    stress_stats = stats(stress)
    risks = [trade["risk_usd"] for trade in trades]
    ob_distances = [trade["ob_distance"] for trade in trades]
    buys = [trade for trade in trades if trade["direction"] == "BUY"]
    sells = [trade for trade in trades if trade["direction"] == "SELL"]

    lines = [
        "# Scalp Standalone M15 OB Retest — research screen",
        "",
        "> Historical in-sample screen. Pre-registration:",
        "> `docs/codexminimal/research/2026-11-19-scalp-m15-standalone-preregistration.md`.",
        "> No live changes.",
        "",
        f"- Data: **{bars['n']}** M15 bars, {bars['symbol']} {bars['server']}",
        f"- Trades: **{len(trades)}** / filter skips htf={skipped['htf']} rsi={skipped['rsi']} ob={skipped['ob']}",
        f"- Trades/week: **{len(trades)/weeks:.2f}**",
        f"- Entry mode: **{entry_mode}**",
        f"- Mean risk: **{sum(risks)/len(risks):.2f} USD/oz**; mean OB distance "
        f"{sum(ob_distances)/len(ob_distances):.2f} ATR",
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
        "## Side split (cost 0.50)",
        "",
        "| Side | n | Win% | Avg R | PF |",
        "|---|---:|---:|---:|---:|",
    ]
    for name, group in (("BUY", buys), ("SELL", sells)):
        summary = stats([trade["r"] for trade in group])
        lines.append(
            f"| {name} | {summary['n']} | {summary['win']} | {summary['avg']:+.3f} | "
            f"{summary['pf']:.2f} |"
        )

    lines.extend(["", "## Yearly (cost 0.50)", ""])
    lines.extend(year_table(trades))
    lines.extend(["", "## Yearly (cost 1.00 stress)", ""])
    lines.extend(year_table(trades, use_stress=True))

    lines.extend(
        [
            "",
            "## Comparison",
            "",
            "| Setup | Alerts/week | Avg R | PF | MaxDD |",
            "|---|---:|---:|---:|---:|",
            "| M15 engine trigger + OB (current) | 1.68 | +0.426 | 2.37 | 8.3 |",
            f"| M15 standalone OB retest ({entry_mode} entry) | {len(trades)/weeks:.2f} | {base['avg']:+.3f} | "
            f"{base['pf']:.2f} | {base['dd']:.1f} |",
            "| M5 standalone OB retest (rejected) | 12.69 | +0.106 | 1.25 | 91.4 |",
            "",
            "## Reading",
            "",
            "- Candidate only if cost 1.0 avg R is positive and PF > 1.2.",
            "- This standalone stream is separate from the validated engine+OB stream.",
            "- Entry sits at the broken level, which is at/below the OB body for SELL (and",
            "  at/above for BUY), so the OB distance is structurally 0 for most signals;",
            "  this screen tests the displacement + retest structure, not a distance cut.",
            "- This is not forward OOS; no promotion without a shadow period.",
            "- If negative or below 2.5 alerts/week, do not add it as a second stream.",
            "",
        ]
    )
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", default="rust-backend/data/backtest/XAUUSDm_M15.jsonl")
    parser.add_argument("--out", default="")
    parser.add_argument("--entry", choices=("level", "close", "close_atr", "confirm"), default="level")
    args = parser.parse_args()

    bars = load_bars(args.cache)
    atr = atr14_series(bars)
    h1 = aggregate(bars, H1_MS)
    h4 = aggregate(bars, H4_MS)
    d1 = aggregate(bars, D1_MS)
    trades, skipped = run(bars, atr, h4, d1, h1, entry_mode=args.entry)
    weeks = (bars["ts"][-1] - bars["ts"][0]) / WEEK_MS
    report = render(bars, trades, weeks, skipped, entry_mode=args.entry)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
