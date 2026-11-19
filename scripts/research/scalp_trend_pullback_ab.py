#!/usr/bin/env python3
"""Scalp trend pullback (Setup A) screen — research only.

Pre-registration:
docs/codexminimal/research/2026-11-20-scalp-trend-pullback-preregistration.md

Slow permission: H1/H4 aligned bias, D1 not opposing, ATR not too low, liquid
session. Fast execution: M5 close rejection/engulfing at the EMA50 M15 zone with
volume >= 1.2x MA20. SL beyond the rejection cluster + 0.5 ATR(M5), TP1 1.5R /
TP2 2.5R, 8-bar time stop if MFE < 0.5R, 96-bar hard timeout.

Usage:
  python3 scripts/research/scalp_trend_pullback_ab.py \
    --cache rust-backend/data/backtest/XAUUSDm_M5.json \
    --out ../docs/SCALP_TREND_PULLBACK_AB_2026-11-20.md
"""

import argparse
import bisect
import importlib.util
import math
import random
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

M5_MS = 5 * 60 * 1000
M15_MS = 15 * 60 * 1000
H1_MS = 60 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
D1_MS = 24 * 60 * 60 * 1000
WEEK_MS = 7 * 24 * 60 * 60 * 1000

COST_USD = 0.5
COST_USD_STRESS = 1.0
COST_USD_HIGH = 2.0
TP1_R = 1.5
TP2_R = 2.5
TIME_STOP_BARS = 8
TIMEOUT_BARS = 96
ATR_LOOKBACK = 576
ATR_MIN_RATIO = 0.60
ZONE_ATR = 0.25
VOLUME_MA = 20
VOLUME_MULT = 1.2
SESSION_START = 7
SESSION_END = 16


def load_module():
    path = Path(__file__).with_name("scalp_m15_standalone_ab.py")
    spec = importlib.util.spec_from_file_location("scalp_m15_standalone_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def weekly_bootstrap(trades, iterations=1000, seed=17):
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


def rolling_mean(values, window):
    out = [0.0] * len(values)
    total = 0.0
    for index, value in enumerate(values):
        total += value
        if index >= window:
            total -= values[index - window]
        out[index] = total / min(index + 1, window)
    return out


def simulate(bars, direction, entry, stop_loss, start_index, cost_usd, be_after_tp1):
    risk = abs(entry - stop_loss)
    if risk <= 0:
        return None
    if direction == "BUY":
        tp1 = entry + TP1_R * risk
        tp2 = entry + TP2_R * risk
    else:
        tp1 = entry - TP1_R * risk
        tp2 = entry - TP2_R * risk
    tp1_hit = False
    mfe = 0.0
    sl = stop_loss
    for offset in range(1, TIMEOUT_BARS + 1):
        index = start_index + offset
        if index >= bars["n"]:
            break
        high, low, close = bars["h"][index], bars["l"][index], bars["c"][index]
        if direction == "BUY":
            mfe = max(mfe, (high - entry) / risk)
        else:
            mfe = max(mfe, (entry - low) / risk)
        if not tp1_hit:
            if direction == "BUY" and low <= sl:
                return -1.0 - cost_usd / risk, offset
            if direction == "SELL" and high >= sl:
                return -1.0 - cost_usd / risk, offset
            if direction == "BUY" and high >= tp1:
                tp1_hit = True
            if direction == "SELL" and low <= tp1:
                tp1_hit = True
            if be_after_tp1 and tp1_hit:
                sl = entry
        else:
            if direction == "BUY" and low <= sl:
                return 0.5 * TP1_R - cost_usd / risk, offset
            if direction == "SELL" and high >= sl:
                return 0.5 * TP1_R - cost_usd / risk, offset
        if tp1_hit:
            if direction == "BUY" and high >= tp2:
                return 0.5 * (TP1_R + TP2_R) - cost_usd / risk, offset
            if direction == "SELL" and low <= tp2:
                return 0.5 * (TP1_R + TP2_R) - cost_usd / risk, offset
        if not tp1_hit and offset == TIME_STOP_BARS and mfe < 0.5:
            close_r = (close - entry) / risk if direction == "BUY" else (entry - close) / risk
            return close_r - cost_usd / risk, offset
    index = min(start_index + TIMEOUT_BARS, bars["n"] - 1)
    close = bars["c"][index]
    close_r = (close - entry) / risk if direction == "BUY" else (entry - close) / risk
    if tp1_hit:
        return 0.5 * TP1_R + 0.5 * close_r - cost_usd / risk, TIMEOUT_BARS
    return close_r - cost_usd / risk, TIMEOUT_BARS


def run(bars, atr, h1, h4, d1, m15, volume_ma, base, be_after_tp1=False, buffer_atr=0.5):
    h1_close = [ts + H1_MS for ts in h1["ts"]]
    h4_close = [ts + H4_MS for ts in h4["ts"]]
    d1_close = [ts + D1_MS for ts in d1["ts"]]
    h1_bias = base.bias_series(h1["c"], base.ema_series(h1["c"], 50), base.ema_series(h1["c"], 200))
    h4_bias = base.bias_series(h4["c"], base.ema_series(h4["c"], 50), base.ema_series(h4["c"], 200))
    d1_bias = base.bias_series(d1["c"], base.ema_series(d1["c"], 50), base.ema_series(d1["c"], 200))
    m15_ema = base.ema_series(m15["c"], 50)
    m15_atr = base.atr14_series(m15)
    m15_close = [ts + M15_MS for ts in m15["ts"]]

    counters = Counter()
    trades = []
    i = max(ATR_LOOKBACK, 200)
    next_free = i
    while i < bars["n"] - 2:
        if i < next_free:
            i += 1
            continue
        hour = datetime.fromtimestamp(bars["ts"][i] / 1000, tz=timezone.utc).hour
        if hour < SESSION_START or hour >= SESSION_END:
            counters["session_blocked"] += 1
            i += 1
            continue
        atr_value = atr[i]
        if atr_value is None or atr_value <= 0:
            i += 1
            continue
        m15_index = base.last_closed_index(m15_close, bars["ts"][i] + M5_MS)
        if m15_index is None or m15_atr[m15_index] is None:
            i += 1
            continue
        zone = ZONE_ATR * m15_atr[m15_index]
        zone_mid = m15_ema[m15_index]
        zone_high = zone_mid + zone
        zone_low = zone_mid - zone

        direction = None
        if (
            bars["l"][i] <= zone_high
            and bars["c"][i] > zone_low
            and bars["c"][i] > bars["o"][i]
        ):
            direction = "BUY"
        elif (
            bars["h"][i] >= zone_low
            and bars["c"][i] < zone_high
            and bars["c"][i] < bars["o"][i]
        ):
            direction = "SELL"
        if direction is None:
            i += 1
            continue
        counters["zone_touch"] += 1

        body = abs(bars["c"][i] - bars["o"][i])
        lower_wick = min(bars["o"][i], bars["c"][i]) - bars["l"][i]
        upper_wick = bars["h"][i] - max(bars["o"][i], bars["c"][i])
        if direction == "BUY":
            rejection = lower_wick >= 2.0 * max(body, 1e-9)
            engulfing = (
                bars["c"][i] > bars["o"][i]
                and bars["o"][i] <= bars["c"][i - 1]
                and bars["c"][i] >= bars["o"][i - 1]
            )
        else:
            rejection = upper_wick >= 2.0 * max(body, 1e-9)
            engulfing = (
                bars["c"][i] < bars["o"][i]
                and bars["o"][i] >= bars["c"][i - 1]
                and bars["c"][i] <= bars["o"][i - 1]
            )
        if not rejection and not engulfing:
            counters["pattern_blocked"] += 1
            i += 1
            continue
        if volume_ma[i] <= 0 or bars["v"][i] < VOLUME_MULT * volume_ma[i]:
            counters["volume_blocked"] += 1
            i += 1
            continue

        signal_time = bars["ts"][i] + M5_MS
        h1_index = base.last_closed_index(h1_close, signal_time)
        h4_index = base.last_closed_index(h4_close, signal_time)
        d1_index = base.last_closed_index(d1_close, signal_time)
        if h1_index is None or h4_index is None or d1_index is None:
            i += 1
            continue
        if direction == "BUY":
            htf_pass = (
                h1_bias[h1_index] == "BULLISH"
                and h4_bias[h4_index] == "BULLISH"
                and d1_bias[d1_index] != "BEARISH"
            )
        else:
            htf_pass = (
                h1_bias[h1_index] == "BEARISH"
                and h4_bias[h4_index] == "BEARISH"
                and d1_bias[d1_index] != "BULLISH"
            )
        if not htf_pass:
            counters["htf_blocked"] += 1
            i += 1
            continue

        window = [value for value in atr[max(0, i - ATR_LOOKBACK + 1) : i + 1] if value]
        if window:
            window.sort()
            median = window[len(window) // 2]
            if atr_value < ATR_MIN_RATIO * median:
                counters["atr_blocked"] += 1
                i += 1
                continue

        if direction == "BUY":
            cluster_low = min(bars["l"][i - 2 : i + 1])
            stop_loss = cluster_low - buffer_atr * atr_value
        else:
            cluster_high = max(bars["h"][i - 2 : i + 1])
            stop_loss = cluster_high + buffer_atr * atr_value
        entry = bars["c"][i]
        if abs(entry - stop_loss) <= 0:
            i += 1
            continue

        result = simulate(bars, direction, entry, stop_loss, i, COST_USD, be_after_tp1)
        result1 = simulate(bars, direction, entry, stop_loss, i, COST_USD_STRESS, be_after_tp1)
        result2 = simulate(bars, direction, entry, stop_loss, i, COST_USD_HIGH, be_after_tp1)
        if result is None or result1 is None or result2 is None:
            i += 1
            continue
        counters["entries"] += 1
        trades.append(
            {
                "ts": bars["ts"][i],
                "month": datetime.fromtimestamp(bars["ts"][i] / 1000, tz=timezone.utc).strftime("%Y-%m"),
                "direction": direction,
                "entry": entry,
                "stop_loss": stop_loss,
                "risk_usd": abs(entry - stop_loss),
                "r": result[0],
                "r1": result1[0],
                "r2": result2[0],
                "bars": result[1],
            }
        )
        next_free = i + result[1] + 1
        i = next_free
    weeks = (bars["ts"][-1] - bars["ts"][0]) / WEEK_MS
    return trades, counters, weeks


def row(name, trades, weeks, base, key="r"):
    summary = base.stats([trade[key] for trade in trades])
    return (
        f"| {name} | {summary['n']} | {summary['n']/weeks:.2f} | {summary['win']} | "
        f"{summary['avg']:+.3f} | {summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} |"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", default="rust-backend/data/backtest/XAUUSDm_M5.json")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    base = load_module()
    bars = base.load_bars(args.cache)
    atr = base.atr14_series(bars)
    h1 = base.aggregate(bars, H1_MS)
    h4 = base.aggregate(bars, H4_MS)
    d1 = base.aggregate(bars, D1_MS)
    m15 = base.aggregate(bars, M15_MS)
    m15["n"] = len(m15["ts"])
    volume_ma = rolling_mean(bars["v"], VOLUME_MA)

    v0, counters, weeks = run(bars, atr, h1, h4, d1, m15, volume_ma, base, False, 0.5)
    v1, _, _ = run(bars, atr, h1, h4, d1, m15, volume_ma, base, True, 0.5)
    v2, _, _ = run(bars, atr, h1, h4, d1, m15, volume_ma, base, False, 0.0)

    primary = base.stats([trade["r"] for trade in v0])
    stress = base.stats([trade["r1"] for trade in v0])
    high = base.stats([trade["r2"] for trade in v0])
    ci = weekly_bootstrap(v0)
    risks = [trade["risk_usd"] for trade in v0]

    lines = [
        "# Scalp Trend Pullback (Setup A) — research screen",
        "",
        "> Pre-registration:",
        "> `docs/codexminimal/research/2026-11-20-scalp-trend-pullback-preregistration.md`.",
        "> Executable M5 close entries. No live changes.",
        "",
        f"- Data: **{bars['n']}** M5 bars, {bars['symbol']} {bars['server']}",
        f"- Weeks: **{weeks:.1f}**",
        f"- Funnel: zone {counters['zone_touch']}, pattern {counters['pattern_blocked']} blocked, "
        f"volume {counters['volume_blocked']} blocked, htf {counters['htf_blocked']} blocked, "
        f"atr {counters['atr_blocked']} blocked, entries {counters['entries']}",
        f"- Mean risk: **{sum(risks)/len(risks):.2f} USD/oz**",
        "",
        "## V0 — no BE, cluster SL + 0.5 ATR",
        "",
        "| Metric | Cost 0.50 | Cost 1.00 | Cost 2.00 |",
        "|---|---:|---:|---:|",
        f"| n | {primary['n']} | {stress['n']} | {high['n']} |",
        f"| Trades/week | {primary['n']/weeks:.2f} | {stress['n']/weeks:.2f} | {high['n']/weeks:.2f} |",
        f"| Win% | {primary['win']} | {stress['win']} | {high['win']} |",
        f"| Avg R | {primary['avg']:+.3f} | {stress['avg']:+.3f} | {high['avg']:+.3f} |",
        f"| Total R | {primary['total']:+.1f} | {stress['total']:+.1f} | {high['total']:+.1f} |",
        f"| PF | {primary['pf']:.2f} | {stress['pf']:.2f} | {high['pf']:.2f} |",
        f"| MaxDD | {primary['dd']:.1f} | {stress['dd']:.1f} | {high['dd']:.1f} |",
        f"| Weekly bootstrap 95% CI (cost 0.50) | [{ci[0]:+.3f}, {ci[1]:+.3f}] | — | — |",
        "",
        "## Variants",
        "",
        "| Variant | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        row("V0 no BE (cost 0.50)", v0, weeks, base),
        row("V1 BE after TP1 (cost 0.50)", v1, weeks, base),
        row("V2 no ATR buffer (cost 0.50)", v2, weeks, base),
        row("V0 no BE (cost 1.00)", v0, weeks, base, "r1"),
        row("V1 BE after TP1 (cost 1.00)", v1, weeks, base, "r1"),
        row("V2 no ATR buffer (cost 1.00)", v2, weeks, base, "r1"),
        "",
        "## Monthly — V0 (cost 0.50)",
        "",
        "| Month | n | Win% | Avg R | PF |",
        "|---|---:|---:|---:|---:|",
    ]
    for month in sorted({trade["month"] for trade in v0}):
        group = [trade["r"] for trade in v0 if trade["month"] == month]
        stats = base.stats(group)
        lines.append(
            f"| {month} | {stats['n']} | {stats['win']} | {stats['avg']:+.3f} | {stats['pf']:.2f} |"
        )

    buys = [trade for trade in v0 if trade["direction"] == "BUY"]
    sells = [trade for trade in v0 if trade["direction"] == "SELL"]
    lines.extend(
        [
            "",
            "## Side split — V0 (cost 0.50)",
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
            "- Cost 2.0 must not be catastrophic (avg R > -0.05R) for a scalp.",
            "- M1 replay 2026 is the next gate before any shadow.",
            "",
        ]
    )
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
