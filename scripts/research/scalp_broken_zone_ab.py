#!/usr/bin/env python3
"""Scalp broken-zone pullback — Setup A (scalp-key) — research only.

Pre-registration:
docs/codexminimal/research/2026-11-20-scalp-broken-zone-preregistration.md

M15 broken zone with >=2 touches before the break, then an M5 close
rejection/engulfing + volume entry on the pullback. SL beyond the 5-bar
rejection cluster + 0.75 ATR(M5), TP1 1.5R / TP2 2.5R, time stop 8 bars,
96-bar hard timeout. Executable M5 close entry.

Usage:
  python3 scripts/research/scalp_broken_zone_ab.py \
    --cache rust-backend/data/backtest/XAUUSDm_M5.json \
    --out ../docs/SCALP_BROKEN_ZONE_AB_2026-11-20.md
"""

import argparse
import bisect
import importlib.util
import math
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
SESSION_START = 7
SESSION_END = 16
SWING_LOOKBACK = 20
TOUCH_LOOKBACK = 60
TOUCH_ATR = 0.25
ZONE_WIDTH_ATR = 0.25
ZONE_TTL_BARS = 96
CLUSTER_BARS = 5
BUFFER_ATR = 0.75
ATR_LOOKBACK = 576
ATR_MIN_RATIO = 0.60
VOLUME_MA = 20
VOLUME_MULT = 1.2


def load_module(name, filename):
    path = Path(__file__).with_name(filename)
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def build_zones(m15, atr15):
    zones = []
    for j in range(TOUCH_LOOKBACK, m15["n"] - 1):
        atr = atr15[j]
        if atr is None or atr <= 0:
            continue
        level = max(m15["h"][j - SWING_LOOKBACK : j])
        if m15["c"][j] > level:
            touches = sum(
                1 for k in range(j - TOUCH_LOOKBACK, j) if abs(m15["h"][k] - level) <= TOUCH_ATR * atr
            )
            if touches >= 2:
                zones.append(
                    {
                        "direction": "BUY",
                        "level": level,
                        "atr": atr,
                        "break_index": j,
                        "touches": touches,
                    }
                )
        level = min(m15["l"][j - SWING_LOOKBACK : j])
        if m15["c"][j] < level:
            touches = sum(
                1 for k in range(j - TOUCH_LOOKBACK, j) if abs(m15["l"][k] - level) <= TOUCH_ATR * atr
            )
            if touches >= 2:
                zones.append(
                    {
                        "direction": "SELL",
                        "level": level,
                        "atr": atr,
                        "break_index": j,
                        "touches": touches,
                    }
                )
    zones.sort(key=lambda zone: zone["break_index"])
    return zones


def run(bars, atr, h1, h4, d1, m15, m15_atr, zones, volume_ma, base, trend, simulate):
    h1_close = [ts + H1_MS for ts in h1["ts"]]
    h4_close = [ts + H4_MS for ts in h4["ts"]]
    d1_close = [ts + D1_MS for ts in d1["ts"]]
    h1_bias = base.bias_series(h1["c"], base.ema_series(h1["c"], 50), base.ema_series(h1["c"], 200))
    h4_bias = base.bias_series(h4["c"], base.ema_series(h4["c"], 50), base.ema_series(h4["c"], 200))
    d1_bias = base.bias_series(d1["c"], base.ema_series(d1["c"], 50), base.ema_series(d1["c"], 200))
    m15_close = [ts + M15_MS for ts in m15["ts"]]
    zone_breaks = [zone["break_index"] for zone in zones]

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
        if m15_index is None:
            i += 1
            continue
        lower = bisect.bisect_left(zone_breaks, m15_index - ZONE_TTL_BARS)
        upper = bisect.bisect_left(zone_breaks, m15_index)
        active = [
            zone
            for zone in zones[lower:upper]
            if zone["break_index"] < m15_index
        ]
        if not active:
            i += 1
            continue

        matched = []
        for zone in active:
            width = ZONE_WIDTH_ATR * zone["atr"]
            zone_high = zone["level"] + width
            zone_low = zone["level"] - width
            if zone["direction"] == "BUY":
                if (
                    bars["l"][i] <= zone_high
                    and bars["c"][i] > zone_low
                    and bars["c"][i] > bars["o"][i]
                ):
                    matched.append(zone)
            else:
                if (
                    bars["h"][i] >= zone_low
                    and bars["c"][i] < zone_high
                    and bars["c"][i] < bars["o"][i]
                ):
                    matched.append(zone)
        if not matched:
            i += 1
            continue
        counters["zone_touch"] += 1
        matched.sort(key=lambda zone: (zone["touches"], zone["break_index"]), reverse=True)
        zone = matched[0]
        direction = zone["direction"]

        if trend:
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

        window = [value for value in atr[max(0, i - ATR_LOOKBACK + 1) : i + 1] if value]
        if window:
            window.sort()
            median = window[len(window) // 2]
            if atr_value < ATR_MIN_RATIO * median:
                counters["atr_blocked"] += 1
                i += 1
                continue

        if direction == "BUY":
            cluster_low = min(bars["l"][i - CLUSTER_BARS + 1 : i + 1])
            stop_loss = cluster_low - BUFFER_ATR * atr_value
        else:
            cluster_high = max(bars["h"][i - CLUSTER_BARS + 1 : i + 1])
            stop_loss = cluster_high + BUFFER_ATR * atr_value
        entry = bars["c"][i]
        if abs(entry - stop_loss) <= 0:
            i += 1
            continue

        result = simulate(bars, direction, entry, stop_loss, i, COST_USD, False)
        result1 = simulate(bars, direction, entry, stop_loss, i, COST_USD_STRESS, False)
        result2 = simulate(bars, direction, entry, stop_loss, i, COST_USD_HIGH, False)
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
                "touches": zone["touches"],
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

    base = load_module("scalp_m15_standalone_ab", "scalp_m15_standalone_ab.py")
    trend = load_module("scalp_trend_pullback_ab", "scalp_trend_pullback_ab.py")
    bars = base.load_bars(args.cache)
    atr = base.atr14_series(bars)
    h1 = base.aggregate(bars, H1_MS)
    h4 = base.aggregate(bars, H4_MS)
    d1 = base.aggregate(bars, D1_MS)
    m15 = base.aggregate(bars, M15_MS)
    m15["n"] = len(m15["ts"])
    m15_atr = base.atr14_series(m15)
    volume_ma = trend.rolling_mean(bars["v"], VOLUME_MA)
    zones = build_zones(m15, m15_atr)

    with_trend, counters, weeks = run(
        bars, atr, h1, h4, d1, m15, m15_atr, zones, volume_ma, base, True, trend.simulate
    )
    without_trend, _, _ = run(
        bars, atr, h1, h4, d1, m15, m15_atr, zones, volume_ma, base, False, trend.simulate
    )

    primary = base.stats([trade["r"] for trade in with_trend])
    stress = base.stats([trade["r1"] for trade in with_trend])
    high = base.stats([trade["r2"] for trade in with_trend])
    ci = trend.weekly_bootstrap(with_trend)
    risks = [trade["risk_usd"] for trade in with_trend]

    lines = [
        "# Scalp Broken-Zone Pullback (scalp-key Setup A) — research screen",
        "",
        "> Pre-registration:",
        "> `docs/codexminimal/research/2026-11-20-scalp-broken-zone-preregistration.md`.",
        "> Executable M5 close entries. No live changes.",
        "",
        f"- Data: **{bars['n']}** M5 bars, {bars['symbol']} {bars['server']}",
        f"- Weeks: **{weeks:.1f}**; zones built: **{len(zones)}**",
        f"- Funnel: zone {counters['zone_touch']}, pattern {counters['pattern_blocked']} blocked, "
        f"volume {counters['volume_blocked']} blocked, htf {counters['htf_blocked']} blocked, "
        f"atr {counters['atr_blocked']} blocked, entries {counters['entries']}",
        f"- Mean risk: **{sum(risks)/len(risks):.2f} USD/oz**",
        "",
        "## Primary — HTF aligned",
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
        "## No-HTF variant",
        "",
        "| Variant | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        row("No HTF (cost 0.50)", without_trend, weeks, base),
        row("No HTF (cost 1.00)", without_trend, weeks, base, "r1"),
        "",
        "## Monthly — primary (cost 0.50)",
        "",
        "| Month | n | Win% | Avg R | PF |",
        "|---|---:|---:|---:|---:|",
    ]
    for month in sorted({trade["month"] for trade in with_trend}):
        group = [trade["r"] for trade in with_trend if trade["month"] == month]
        stats = base.stats(group)
        lines.append(
            f"| {month} | {stats['n']} | {stats['win']} | {stats['avg']:+.3f} | {stats['pf']:.2f} |"
        )

    buys = [trade for trade in with_trend if trade["direction"] == "BUY"]
    sells = [trade for trade in with_trend if trade["direction"] == "SELL"]
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
            "- Then M1 replay 2026 must keep the sign.",
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
