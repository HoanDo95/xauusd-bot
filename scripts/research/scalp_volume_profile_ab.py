#!/usr/bin/env python3
"""Scalp volume profile screen (research only).

Pre-registration:
docs/codexminimal/research/2026-11-19-scalp-volume-profile-preregistration.md

Two tests on executable close entries:
  H1 - VP confluence filter on the standalone M15 OB retest.
  H2 - standalone VP rejection at VAL / VAH.

Usage:
  python3 scripts/research/scalp_volume_profile_ab.py \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_VOLUME_PROFILE_AB_2026-11-19.md
"""

import argparse
import bisect
import importlib.util
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
TIMEOUT_BARS = 96
BUFFER_USD = 3.0
VP_LOOKBACK = 96
VP_BINS = 50
VP_VALUE_AREA = 0.70
VP_CONFLUENCE_ATR = 0.25


def load_module():
    path = Path(__file__).with_name("scalp_m15_standalone_ab.py")
    spec = importlib.util.spec_from_file_location("scalp_m15_standalone_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def volume_profile(o, h, l, v, start, end, bins=VP_BINS):
    lo = min(l[start:end])
    hi = max(h[start:end])
    if hi <= lo:
        return None
    width = (hi - lo) / bins
    buckets = [0.0] * bins
    for k in range(start, end):
        bar_lo, bar_hi, bar_vol = l[k], h[k], v[k]
        if bar_vol <= 0:
            continue
        if bar_hi <= bar_lo:
            idx = int((bar_lo - lo) / width)
            if idx < 0:
                idx = 0
            elif idx >= bins:
                idx = bins - 1
            buckets[idx] += bar_vol
            continue
        first = int((bar_lo - lo) / width)
        last = int((bar_hi - lo) / width)
        if first < 0:
            first = 0
        if last >= bins:
            last = bins - 1
        span = bar_hi - bar_lo
        for b in range(first, last + 1):
            bin_lo = lo + b * width
            bin_hi = bin_lo + width
            overlap = min(bar_hi, bin_hi) - max(bar_lo, bin_lo)
            if overlap > 0:
                buckets[b] += bar_vol * overlap / span
    total = sum(buckets)
    if total <= 0:
        return None
    poc_idx = 0
    for b in range(1, bins):
        if buckets[b] > buckets[poc_idx]:
            poc_idx = b
    low_idx = high_idx = poc_idx
    acc = buckets[poc_idx]
    target = total * VP_VALUE_AREA
    while acc < target and (low_idx > 0 or high_idx < bins - 1):
        left = buckets[low_idx - 1] if low_idx > 0 else -1.0
        right = buckets[high_idx + 1] if high_idx < bins - 1 else -1.0
        if right >= left:
            high_idx += 1
            acc += buckets[high_idx]
        else:
            low_idx -= 1
            acc += buckets[low_idx]
    return (
        lo + (poc_idx + 0.5) * width,
        lo + low_idx * width,
        lo + (high_idx + 1) * width,
    )


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


def row(name, trades, weeks, key="r"):
    values = [trade[key] for trade in trades]
    summary = stats(values)
    per_week = summary["n"] / weeks if weeks else 0.0
    return (
        f"| {name} | {summary['n']} | {per_week:.2f} | {summary['win']} | "
        f"{summary['avg']:+.3f} | {summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} |"
    )


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
    weeks = (bars["ts"][-1] - bars["ts"][0]) / WEEK_MS

    # ---------- H1: VP confluence on the executable retest ----------
    trades, skipped = base.run(bars, atr, h4, d1, h1, entry_mode="close")
    enriched = []
    for trade in trades:
        i = trade["index"]
        if i < VP_LOOKBACK + 1:
            continue
        profile = volume_profile(
            bars["o"], bars["h"], bars["l"], bars["v"], i - VP_LOOKBACK, i
        )
        if not profile:
            continue
        poc, val, vah = profile
        atr_value = atr[i]
        if not atr_value or atr_value <= 0:
            continue
        level = trade["level"]
        distance = min(abs(level - poc), abs(level - vah), abs(level - val)) / atr_value
        trade = dict(trade)
        trade["confluence"] = distance <= VP_CONFLUENCE_ATR
        trade["inside_va"] = val <= level <= vah
        trade["distance"] = distance
        enriched.append(trade)

    confluence = [t for t in enriched if t["confluence"]]
    non_confluence = [t for t in enriched if not t["confluence"]]
    inside = [t for t in enriched if t["inside_va"]]
    outside = [t for t in enriched if not t["inside_va"]]

    lines = [
        "# Scalp Volume Profile — research screen",
        "",
        "> Pre-registration:",
        "> `docs/codexminimal/research/2026-11-19-scalp-volume-profile-preregistration.md`.",
        "> Executable close entries only. No live changes.",
        "",
        f"- Data: **{bars['n']}** M15 bars, 2022-08 → 2026-11",
        f"- VP: {VP_LOOKBACK}-bar lookback, {VP_BINS} bins, {int(VP_VALUE_AREA*100)}% value area",
        "- Confluence: level within 0.25 ATR of POC/VAH/VAL",
        "",
        "## H1 — VP confluence on OB retest (entry = close)",
        "",
        "| Group | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        row("All close-entry retests", enriched, weeks),
        row("VP confluence (<=0.25 ATR)", confluence, weeks),
        row("No confluence", non_confluence, weeks),
        row("Inside value area", inside, weeks),
        row("Outside value area", outside, weeks),
        "",
        "### H1 cost stress 1.00",
        "",
        "| Group | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        row("All close-entry retests", enriched, weeks, "r1"),
        row("VP confluence (<=0.25 ATR)", confluence, weeks, "r1"),
        row("No confluence", non_confluence, weeks, "r1"),
        row("Inside value area", inside, weeks, "r1"),
        row("Outside value area", outside, weeks, "r1"),
    ]

    # ---------- H2: VP rejection setup ----------
    vp_trades = []
    j = max(VP_LOOKBACK + 20, 20)
    next_free = j
    while j < bars["n"] - 2:
        if j < next_free:
            j += 1
            continue
        if atr[j] is None or atr[j] <= 0:
            j += 1
            continue
        profile = volume_profile(
            bars["o"], bars["h"], bars["l"], bars["v"], j - VP_LOOKBACK, j
        )
        if not profile:
            j += 1
            continue
        poc, val, vah = profile
        direction = None
        if (
            bars["l"][j] <= val
            and bars["c"][j] > val
            and bars["c"][j] > bars["o"][j]
        ):
            direction = "BUY"
        elif (
            bars["h"][j] >= vah
            and bars["c"][j] < vah
            and bars["c"][j] < bars["o"][j]
        ):
            direction = "SELL"
        if direction is None:
            j += 1
            continue
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
            j += 1
            continue
        rsi_index = base.last_closed_index(h1_close, signal_time)
        rsi_value = h1_rsi[rsi_index] if rsi_index is not None else None
        if direction == "BUY" and rsi_value is not None and rsi_value < 30.0:
            j += 1
            continue
        entry = bars["c"][j]
        stop_loss = (
            bars["h"][j] + BUFFER_USD if direction == "SELL" else bars["l"][j] - BUFFER_USD
        )
        result = base.simulate(bars, direction, entry, stop_loss, j, COST_USD)
        result1 = base.simulate(bars, direction, entry, stop_loss, j, COST_USD_STRESS)
        if result is None or result1 is None:
            j += 1
            continue
        vp_trades.append(
            {
                "time": signal_time,
                "year": datetime.fromtimestamp(signal_time / 1000, tz=timezone.utc).year,
                "direction": direction,
                "entry": entry,
                "stop_loss": stop_loss,
                "r": result[0],
                "r1": result1[0],
                "bars": result[1],
            }
        )
        next_free = j + result[1] + 1
        j = next_free

    lines.extend(
        [
            "",
            "## H2 — VP rejection setup (entry = close)",
            "",
            "| Metric | Cost 0.50 | Cost 1.00 |",
            "|---|---:|---:|",
        ]
    )
    values = [trade["r"] for trade in vp_trades]
    stress = [trade["r1"] for trade in vp_trades]
    summary = stats(values)
    summary_stress = stats(stress)
    lines.extend(
        [
            f"| n | {summary['n']} | {summary_stress['n']} |",
            f"| Trades/week | {summary['n']/weeks:.2f} | {summary_stress['n']/weeks:.2f} |",
            f"| Win% | {summary['win']} | {summary_stress['win']} |",
            f"| Avg R | {summary['avg']:+.3f} | {summary_stress['avg']:+.3f} |",
            f"| Total R | {summary['total']:+.1f} | {summary_stress['total']:+.1f} |",
            f"| PF | {summary['pf']:.2f} | {summary_stress['pf']:.2f} |",
            f"| MaxDD | {summary['dd']:.1f} | {summary_stress['dd']:.1f} |",
            "",
            "### H2 yearly (cost 0.50)",
            "",
            "| Year | n | Win% | Avg R | PF |",
            "|---|---:|---:|---:|---:|",
        ]
    )
    for year in sorted({trade["year"] for trade in vp_trades}):
        group = [trade["r"] for trade in vp_trades if trade["year"] == year]
        stats_year = stats(group)
        lines.append(
            f"| {year} | {stats_year['n']} | {stats_year['win']} | "
            f"{stats_year['avg']:+.3f} | {stats_year['pf']:.2f} |"
        )

    lines.extend(
        [
            "",
            "## Reading",
            "",
            "- H1 passes only if the confluence subset is positive at cost 1.0 and PF > 1.2.",
            "- H2 passes only if avg R at cost 1.0 > 0, PF > 1.2, and >= 2.5 trades/week.",
            "- VP context can still be shown on the dashboard even if no filter passes.",
            "",
        ]
    )
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
