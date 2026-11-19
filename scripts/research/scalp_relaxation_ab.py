#!/usr/bin/env python3
"""Scalp filter relaxation screen.

Exploratory product/frequency screen on the raw retest triggers. Compares the
current scalp filters (HTF + RSI guard + confidence >= 60 + non-COUNTER macro)
against controlled relaxations. Uses the scalp plan: TP1 1R / TP2 2R, 50/50,
BE after TP1, 96-bar timeout.

Not a pre-registered promotion test; no live changes.

Usage:
  python3 scripts/research/scalp_relaxation_ab.py \
    --triggers rust-backend/data/backtest/exness_raw_triggers_v3.jsonl \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_RELAXATION_AB_2026-11-19.md
"""

import argparse
import importlib.util
import json
import math
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path

M15_MS = 15 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
D1_MS = 24 * 60 * 60 * 1000
COST_USD = 0.5
COST_USD_STRESS = 1.0
TIMEOUT_BARS = 96


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


def bias_at(series, ema50, ema200, timestamp_ms, tf_ms):
    index = None
    for i, candle in enumerate(series):
        if candle["timestamp"] + tf_ms <= timestamp_ms:
            index = i
        else:
            break
    if index is None:
        return "UNKNOWN"
    close = series[index]["close"]
    fast = ema50[index]
    slow = ema200[index]
    if close > fast > slow:
        return "BULLISH"
    if close < fast < slow:
        return "BEARISH"
    return "NEUTRAL"


def htf_ok(direction, h4_bias, d1_bias):
    if direction == "SELL":
        return h4_bias != "BULLISH" and d1_bias != "BULLISH"
    if direction == "BUY":
        return h4_bias != "BEARISH" and d1_bias != "BEARISH"
    return False


def rsi_ok(direction, rsi_h1):
    if direction != "BUY":
        return True
    if rsi_h1 is None:
        return True
    return rsi_h1 >= 30.0


def macro_ok(alignment):
    return (alignment or "").upper() != "COUNTER"


def simulate_scalp(direction, entry, stop_loss, start_index, candles, cost_usd):
    risk = abs(entry - stop_loss)
    if risk <= 0 or start_index >= len(candles):
        return None
    if direction == "SELL":
        tp1 = entry - risk
        tp2 = entry - 2.0 * risk
    elif direction == "BUY":
        tp1 = entry + risk
        tp2 = entry + 2.0 * risk
    else:
        return None
    tp1_hit = False
    for offset in range(1, TIMEOUT_BARS + 1):
        index = start_index + offset
        if index >= len(candles):
            break
        candle = candles[index]
        if not tp1_hit:
            if direction == "SELL":
                if candle["high"] >= stop_loss:
                    return -1.0 - cost_usd / risk
                if candle["low"] <= tp1:
                    tp1_hit = True
            else:
                if candle["low"] <= stop_loss:
                    return -1.0 - cost_usd / risk
                if candle["high"] >= tp1:
                    tp1_hit = True
        else:
            # Break-even stop on later candles.
            if direction == "SELL" and candle["high"] >= entry:
                return 0.5 - cost_usd / risk
            if direction == "BUY" and candle["low"] <= entry:
                return 0.5 - cost_usd / risk
        if tp1_hit:
            if direction == "SELL" and candle["low"] <= tp2:
                return 1.5 - cost_usd / risk
            if direction == "BUY" and candle["high"] >= tp2:
                return 1.5 - cost_usd / risk
    index = min(start_index + TIMEOUT_BARS, len(candles) - 1)
    close = candles[index]["close"]
    if direction == "SELL":
        close_r = (entry - close) / risk
    else:
        close_r = (close - entry) / risk
    if tp1_hit:
        return 0.5 + 0.5 * close_r - cost_usd / risk
    return close_r - cost_usd / risk


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


def build_rows(triggers, candles, h4, d1, ema):
    h4_ema50 = ema_series([candle["close"] for candle in h4], 50)
    h4_ema200 = ema_series([candle["close"] for candle in h4], 200)
    d1_ema50 = ema_series([candle["close"] for candle in d1], 50)
    d1_ema200 = ema_series([candle["close"] for candle in d1], 200)
    rows = []
    skipped = 0
    for trigger in triggers:
        if not trigger.get("retest_ok"):
            continue
        direction = trigger.get("direction")
        entry = trigger.get("retest_entry")
        stop_loss = trigger.get("retest_stop_loss")
        retest_time = trigger.get("retest_time")
        if direction not in ("BUY", "SELL") or not entry or not stop_loss or not retest_time:
            skipped += 1
            continue
        retest_ms = parse_ms(retest_time)
        if retest_ms is None:
            skipped += 1
            continue
        index = next(
            (
                i
                for i, candle in enumerate(candles)
                if candle["timestamp"] == retest_ms - M15_MS
            ),
            None,
        )
        if index is None:
            skipped += 1
            continue
        result = simulate_scalp(direction, entry, stop_loss, index, candles, COST_USD)
        result1 = simulate_scalp(
            direction, entry, stop_loss, index, candles, COST_USD_STRESS
        )
        if result is None or result1 is None:
            skipped += 1
            continue
        rows.append(
            {
                "date": retest_time[:10],
                "scenario": trigger.get("scenario_id", "unknown"),
                "direction": direction,
                "confidence": trigger.get("confidence") or 0,
                "rsi_h1": trigger.get("rsi_h1"),
                "macro": trigger.get("macro_alignment"),
                "h4_bias": bias_at(h4, h4_ema50, h4_ema200, retest_ms, H4_MS),
                "d1_bias": bias_at(d1, d1_ema50, d1_ema200, retest_ms, D1_MS),
                "r": result,
                "r1": result1,
            }
        )
    return rows, skipped


def select(
    rows,
    htf=True,
    rsi=True,
    min_confidence=60,
    primary_only=True,
    include_bounce=False,
    only_bounce=False,
):
    selected = []
    for row in rows:
        if only_bounce:
            if row["scenario"] != "bullish_bounce":
                continue
        elif primary_only and not include_bounce and row["scenario"] == "bullish_bounce":
            continue
        if row["confidence"] < min_confidence:
            continue
        if not macro_ok(row["macro"]):
            continue
        if htf and not htf_ok(row["direction"], row["h4_bias"], row["d1_bias"]):
            continue
        if rsi and not rsi_ok(row["direction"], row["rsi_h1"]):
            continue
        selected.append(row)
    return selected


def table_row(lines, name, rows, weeks, use_stress=False):
    values = [row["r1"] if use_stress else row["r"] for row in rows]
    summary = stats(values)
    per_week = summary["n"] / weeks if weeks > 0 else 0.0
    lines.append(
        f"| {name} | {summary['n']} | {per_week:.2f} | {summary['win']} | "
        f"{summary['avg']:+.3f} | {summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} |"
    )


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
    args = parser.parse_args()

    ab = load_ab_module()
    candles = ab.load_candles(args.cache)
    triggers = ab.load_triggers(args.triggers)
    h4 = aggregate_tf(candles, H4_MS)
    d1 = aggregate_tf(candles, D1_MS)
    rows, skipped = build_rows(triggers, candles, h4, d1, None)
    weeks = (candles[-1]["timestamp"] - candles[0]["timestamp"]) / (7 * 24 * 60 * 60 * 1000)

    variants = [
        ("baseline (HTF+RSI+conf60)", dict(htf=True, rsi=True, min_confidence=60)),
        ("no HTF (RSI+conf60)", dict(htf=False, rsi=True, min_confidence=60)),
        ("no RSI (HTF+conf60)", dict(htf=True, rsi=False, min_confidence=60)),
        ("no HTF+RSI (conf60)", dict(htf=False, rsi=False, min_confidence=60)),
        ("relaxed (no HTF+RSI, conf50)", dict(htf=False, rsi=False, min_confidence=50)),
        (
            "all scenarios (HTF+RSI+conf60)",
            dict(htf=True, rsi=True, min_confidence=60, include_bounce=True),
        ),
        (
            "all scenarios relaxed",
            dict(
                htf=False,
                rsi=False,
                min_confidence=50,
                include_bounce=True,
            ),
        ),
        (
            "bounce only (HTF+RSI+conf60)",
            dict(htf=True, rsi=True, min_confidence=60, only_bounce=True),
        ),
    ]

    lines = [
        "# Scalp Relaxation A/B — 2026-11-19",
        "",
        "> Exploratory frequency screen on raw retest triggers. Not a pre-registered",
        "> promotion test. No live changes. Plan: TP1 1R / TP2 2R, 50/50, BE after TP1,",
        "> 96-bar timeout, cost 0.50 USD/oz (stress 1.00).",
        "",
        f"- Retest rows: **{len(rows)}** used / {skipped} skipped",
        f"- Weeks: **{weeks:.1f}**; trades/week = n / weeks",
        "- `primary_only` excludes `bullish_bounce` (alternate-like) as a proxy for the",
        "  scalp worker's PRIMARY-only rule.",
        "",
        "## Results — cost 0.50 USD/oz",
        "",
        "| Variant | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    selected_map = {}
    for name, options in variants:
        selected = select(rows, **options)
        selected_map[name] = selected
        table_row(lines, name, selected, weeks)

    lines.extend(
        [
            "",
            "## Cost stress — 1.00 USD/oz",
            "",
            "| Variant | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
            "|---|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for name, _ in variants:
        table_row(lines, name, selected_map[name], weeks, use_stress=True)

    lines.extend(
        [
            "",
            "## Reading the result",
            "",
            "- Goal: more trades/week without turning avg R/PF negative.",
            "- If `no HTF` or `no RSI` keeps PF > 1.2 and avg R positive, it is a candidate",
            "  for a controlled scalp default change.",
            "- If relaxed variants lose PF/avg R, keep the current filters and add a second",
            "  setup instead of loosening further.",
            "- This is exploratory; any default change needs a forward shadow period.",
            "",
        ]
    )

    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
