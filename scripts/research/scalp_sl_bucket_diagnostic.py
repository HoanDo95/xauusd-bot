#!/usr/bin/env python3
"""SL-bucket diagnostic for the scalp Setup A screens (research only).

Pre-registration:
docs/codexminimal/research/2026-11-20-scalp-sl-bucket-diagnostic-preregistration.md

Buckets are fixed before looking at the data: <4, 4-8, 8-15, >15 USD.
Reports avg R at cost 0 and cost 1.0 with weekly block bootstrap CIs.

Usage:
  python3 scripts/research/scalp_sl_bucket_diagnostic.py \
    --cache rust-backend/data/backtest/XAUUSDm_M5.json \
    --out ../docs/SCALP_SL_BUCKET_DIAGNOSTIC_2026-11-20.md
"""

import argparse
import importlib.util
import math
import random
from pathlib import Path

H1_MS = 60 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
D1_MS = 24 * 60 * 60 * 1000
M15_MS = 15 * 60 * 1000
WEEK_MS = 7 * 24 * 60 * 60 * 1000
BUCKETS = [
    ("< 4 USD", 0.0, 4.0),
    ("4 – < 8 USD", 4.0, 8.0),
    ("8 – 15 USD", 8.0, 15.0),
    ("> 15 USD", 15.0, float("inf")),
]


def load_module(name, filename):
    path = Path(__file__).with_name(filename)
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def stats(values):
    if not values:
        return {"n": 0, "win": 0.0, "avg": 0.0, "pf": 0.0, "dd": 0.0}
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
        "pf": round(gross / loss, 2) if loss > 0 else math.inf,
        "dd": round(dd, 1),
    }


def bootstrap_ci(trades, key, iterations=1500, seed=23):
    if not trades:
        return (0.0, 0.0)
    weeks = {}
    for trade in trades:
        weeks.setdefault(trade["ts"] // WEEK_MS, []).append(trade[key])
    keys = list(weeks)
    rng = random.Random(seed)
    means = []
    for _ in range(iterations):
        values = []
        for _ in keys:
            values.extend(weeks[rng.choice(keys)])
        if values:
            means.append(sum(values) / len(values))
    if not means:
        return (0.0, 0.0)
    means.sort()
    return (
        round(means[int(0.025 * len(means))], 3),
        round(means[int(0.975 * len(means))], 3),
    )


def enrich(trades, bars, trend, index_by_ts):
    enriched = []
    for trade in trades:
        index = index_by_ts.get(trade["ts"])
        if index is None:
            continue
        result = trend.simulate(
            bars,
            trade["direction"],
            trade["entry"],
            trade["stop_loss"],
            index,
            0.0,
            False,
        )
        if result is None:
            continue
        enriched.append({**trade, "r0": result[0]})
    return enriched


def bucket_rows(trades, label):
    lines = []
    overall = stats([trade["r0"] for trade in trades])
    overall1 = stats([trade["r1"] for trade in trades])
    ci0 = bootstrap_ci(trades, "r0")
    ci1 = bootstrap_ci(trades, "r1")
    weeks = len({trade["ts"] // WEEK_MS for trade in trades})
    lines.append(
        f"| **{label} — all** | {len(trades)} | {weeks} | {overall['avg']:+.3f} "
        f"[{ci0[0]:+.3f}, {ci0[1]:+.3f}] | {overall1['avg']:+.3f} "
        f"[{ci1[0]:+.3f}, {ci1[1]:+.3f}] | {overall1['pf']:.2f} |"
    )
    for name, low, high in BUCKETS:
        group = [trade for trade in trades if low <= trade["risk_usd"] < high]
        if not group:
            continue
        s0 = stats([trade["r0"] for trade in group])
        s1 = stats([trade["r1"] for trade in group])
        b0 = bootstrap_ci(group, "r0")
        b1 = bootstrap_ci(group, "r1")
        group_weeks = len({trade["ts"] // WEEK_MS for trade in group})
        lines.append(
            f"| {name} | {len(group)} | {group_weeks} | {s0['avg']:+.3f} "
            f"[{b0[0]:+.3f}, {b0[1]:+.3f}] | {s1['avg']:+.3f} "
            f"[{b1[0]:+.3f}, {b1[1]:+.3f}] | {s1['pf']:.2f} |"
        )
    return lines


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache", default="rust-backend/data/backtest/XAUUSDm_M5.json")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    base = load_module("scalp_m15_standalone_ab", "scalp_m15_standalone_ab.py")
    trend = load_module("scalp_trend_pullback_ab", "scalp_trend_pullback_ab.py")
    broken = load_module("scalp_broken_zone_ab", "scalp_broken_zone_ab.py")

    bars = base.load_bars(args.cache)
    index_by_ts = {ts: i for i, ts in enumerate(bars["ts"])}
    atr = base.atr14_series(bars)
    h1 = base.aggregate(bars, H1_MS)
    h4 = base.aggregate(bars, H4_MS)
    d1 = base.aggregate(bars, D1_MS)
    m15 = base.aggregate(bars, M15_MS)
    m15["n"] = len(m15["ts"])
    m15_atr = base.atr14_series(m15)
    volume_ma = trend.rolling_mean(bars["v"], trend.VOLUME_MA)

    zones = broken.build_zones(m15, m15_atr)
    broken_trades, _, weeks = broken.run(
        bars, atr, h1, h4, d1, m15, m15_atr, zones, volume_ma, base, True, trend.simulate
    )
    broken_trades = enrich(broken_trades, bars, trend, index_by_ts)

    ema_trades, _, _ = trend.run(
        bars, atr, h1, h4, d1, m15, volume_ma, base, False, 0.5
    )
    ema_trades = enrich(ema_trades, bars, trend, index_by_ts)

    harmonic_broken = len(broken_trades) / sum(1.0 / trade["risk_usd"] for trade in broken_trades)
    harmonic_ema = len(ema_trades) / sum(1.0 / trade["risk_usd"] for trade in ema_trades)

    lines = [
        "# Scalp Setup A — SL-bucket diagnostic",
        "",
        "> Pre-registration:",
        "> `docs/codexminimal/research/2026-11-20-scalp-sl-bucket-diagnostic-preregistration.md`.",
        "> Buckets fixed before looking: <4, 4-8, 8-15, >15 USD. Cost 0 and 1.00.",
        "",
        f"- Broken-zone trades: **{len(broken_trades)}**; harmonic mean risk "
        f"**{harmonic_broken:.2f} USD**",
        f"- EMA-zone trades: **{len(ema_trades)}**; harmonic mean risk "
        f"**{harmonic_ema:.2f} USD**",
        "- CI is a weekly block bootstrap 95% (1500 resamples).",
        "",
        "## Broken zone + >=2 touches (primary)",
        "",
        "| Bucket | n | Weeks | Avg R cost 0 [CI] | Avg R cost 1.0 [CI] | PF 1.0 |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    lines.extend(bucket_rows(broken_trades, "Broken zone"))
    lines.extend(
        [
            "",
            "## EMA zone (secondary)",
            "",
            "| Bucket | n | Weeks | Avg R cost 0 [CI] | Avg R cost 1.0 [CI] | PF 1.0 |",
            "|---|---:|---:|---:|---:|---:|",
        ]
    )
    lines.extend(bucket_rows(ema_trades, "EMA zone"))
    lines.extend(
        [
            "",
            "## Reading",
            "",
            "- Effective 1R = harmonic mean risk; mean risk hides the small-SL tail.",
            "- If no bucket is positive at cost 1.0 with CI lower bound > 0, close XAU M5.",
            "- A positive bucket is post-hoc and needs the 2026 M1 replay before any trust.",
            "",
        ]
    )
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
