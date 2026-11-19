#!/usr/bin/env python3
"""Scalp liquidity sweep reversal screen (research only).

Pre-registration:
docs/codexminimal/research/2026-11-20-scalp-liquidity-sweep-preregistration.md

Sweep of the prior 20-bar extreme, close back inside, entry at the close.
Structural SL beyond the sweep extreme + 3 USD, TP1 1R / TP2 2R, 50/50,
BE after TP1, 96-bar timeout. Executable close entry.

Usage:
  python3 scripts/research/scalp_liquidity_sweep_ab.py --tf M15 \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_LIQUIDITY_SWEEP_AB_2026-11-20.md
"""

import argparse
import bisect
import importlib.util
import random
from pathlib import Path

M15_MS = 15 * 60 * 1000
M5_MS = 5 * 60 * 1000
H1_MS = 60 * 60 * 1000
H4_MS = 4 * 60 * 60 * 1000
D1_MS = 24 * 60 * 60 * 1000
WEEK_MS = 7 * 24 * 60 * 60 * 1000
COST_USD = 0.5
COST_USD_STRESS = 1.0
BUFFER_USD = 3.0
SWEEP_LOOKBACK = 20
SWEEP_MIN_ATR = 0.10


def load_module():
    path = Path(__file__).with_name("scalp_m15_standalone_ab.py")
    spec = importlib.util.spec_from_file_location("scalp_m15_standalone_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def weekly_bootstrap(trades, iterations=1000, seed=11):
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


def simulate(bars, tf_ms, h4, d1, h1, use_htf=True):
    base = load_module()
    h4_close = [ts + H4_MS for ts in h4["ts"]]
    d1_close = [ts + D1_MS for ts in d1["ts"]]
    h1_close = [ts + H1_MS for ts in h1["ts"]]
    h4_bias = base.bias_series(h4["c"], base.ema_series(h4["c"], 50), base.ema_series(h4["c"], 200))
    d1_bias = base.bias_series(d1["c"], base.ema_series(d1["c"], 50), base.ema_series(d1["c"], 200))
    h1_rsi = base.rsi_series(h1["c"])
    atr = base.atr14_series(bars)

    trades = []
    i = SWEEP_LOOKBACK + 15
    while i < bars["n"] - 1:
        prior_high = max(bars["h"][i - SWEEP_LOOKBACK : i])
        prior_low = min(bars["l"][i - SWEEP_LOOKBACK : i])
        atr_value = atr[i]
        if atr_value is None or atr_value <= 0:
            i += 1
            continue
        direction = None
        if (
            bars["h"][i] > prior_high
            and bars["c"][i] < prior_high
            and (bars["h"][i] - prior_high) >= SWEEP_MIN_ATR * atr_value
            and bars["c"][i] < bars["o"][i]
        ):
            direction = "SELL"
        elif (
            bars["l"][i] < prior_low
            and bars["c"][i] > prior_low
            and (prior_low - bars["l"][i]) >= SWEEP_MIN_ATR * atr_value
            and bars["c"][i] > bars["o"][i]
        ):
            direction = "BUY"
        if direction is None:
            i += 1
            continue
        signal_time = bars["ts"][i] + tf_ms
        if use_htf:
            h4_index = base.last_closed_index(h4_close, signal_time)
            d1_index = base.last_closed_index(d1_close, signal_time)
            if h4_index is None or d1_index is None:
                i += 1
                continue
            htf_pass = (
                h4_bias[h4_index] != "BULLISH" and d1_bias[d1_index] != "BULLISH"
                if direction == "SELL"
                else h4_bias[h4_index] != "BEARISH" and d1_bias[d1_index] != "BEARISH"
            )
            if not htf_pass:
                i += 1
                continue
        rsi_index = base.last_closed_index(h1_close, signal_time)
        rsi_value = h1_rsi[rsi_index] if rsi_index is not None else None
        if direction == "BUY" and rsi_value is not None and rsi_value < 30.0:
            i += 1
            continue
        entry = bars["c"][i]
        stop_loss = (
            bars["h"][i] + BUFFER_USD if direction == "SELL" else bars["l"][i] - BUFFER_USD
        )
        result = base.simulate(bars, direction, entry, stop_loss, i, COST_USD)
        result1 = base.simulate(bars, direction, entry, stop_loss, i, COST_USD_STRESS)
        if result is None or result1 is None:
            i += 1
            continue
        trades.append(
            {
                "ts": bars["ts"][i],
                "month": __import__("datetime").datetime.utcfromtimestamp(
                    bars["ts"][i] / 1000
                ).strftime("%Y-%m"),
                "year": __import__("datetime").datetime.utcfromtimestamp(
                    bars["ts"][i] / 1000
                ).year,
                "direction": direction,
                "entry": entry,
                "stop_loss": stop_loss,
                "risk_usd": abs(entry - stop_loss),
                "r": result[0],
                "r1": result1[0],
                "bars": result[1],
            }
        )
        i = i + result[1] + 1
    weeks = (bars["ts"][-1] - bars["ts"][0]) / WEEK_MS
    return trades, weeks


def row(name, trades, weeks, key="r"):
    from_string = load_module()
    summary = from_string.stats([trade[key] for trade in trades])
    return (
        f"| {name} | {summary['n']} | {summary['n']/weeks:.2f} | {summary['win']} | "
        f"{summary['avg']:+.3f} | {summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} |"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tf", choices=("M15", "M5"), default="M15")
    parser.add_argument("--cache", default="")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    base = load_module()
    tf_ms = M15_MS if args.tf == "M15" else M5_MS
    cache = args.cache or (
        "rust-backend/data/backtest/XAUUSDm_M15.jsonl"
        if args.tf == "M15"
        else "rust-backend/data/backtest/XAUUSDm_M5.json"
    )
    bars = base.load_bars(cache)
    h1 = base.aggregate(bars, H1_MS)
    h4 = base.aggregate(bars, H4_MS)
    d1 = base.aggregate(bars, D1_MS)

    primary, weeks = simulate(bars, tf_ms, h4, d1, h1, use_htf=True)
    secondary, _ = simulate(bars, tf_ms, h4, d1, h1, use_htf=False)
    primary_stats = base.stats([trade["r"] for trade in primary])
    primary_stress = base.stats([trade["r1"] for trade in primary])
    ci = weekly_bootstrap(primary)
    risks = [trade["risk_usd"] for trade in primary]

    lines = [
        f"# Scalp Liquidity Sweep Reversal — {args.tf} screen",
        "",
        "> Pre-registration:",
        "> `docs/codexminimal/research/2026-11-20-scalp-liquidity-sweep-preregistration.md`.",
        "> Executable close entries only. No live changes.",
        "",
        f"- Data: **{bars['n']}** {args.tf} bars, {bars['symbol']} {bars['server']}",
        f"- Weeks: **{weeks:.1f}**",
        f"- Primary armed: **{len(primary)}** trades",
        f"- Mean risk: **{sum(risks)/len(risks):.2f} USD/oz**",
        "",
        "## Primary — HTF filter on",
        "",
        "| Metric | Cost 0.50 | Cost 1.00 |",
        "|---|---:|---:|",
        f"| n | {primary_stats['n']} | {primary_stress['n']} |",
        f"| Trades/week | {primary_stats['n']/weeks:.2f} | {primary_stress['n']/weeks:.2f} |",
        f"| Win% | {primary_stats['win']} | {primary_stress['win']} |",
        f"| Avg R | {primary_stats['avg']:+.3f} | {primary_stress['avg']:+.3f} |",
        f"| Total R | {primary_stats['total']:+.1f} | {primary_stress['total']:+.1f} |",
        f"| PF | {primary_stats['pf']:.2f} | {primary_stress['pf']:.2f} |",
        f"| MaxDD | {primary_stats['dd']:.1f} | {primary_stress['dd']:.1f} |",
        f"| Weekly bootstrap 95% CI (cost 0.50) | [{ci[0]:+.3f}, {ci[1]:+.3f}] | — |",
        "",
        "## Secondary — no HTF filter",
        "",
        "| Setup | n | Trades/week | Win% | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
        row("No HTF (cost 0.50)", secondary, weeks),
        row("No HTF (cost 1.00)", secondary, weeks, "r1"),
        "",
        "## Monthly — primary (cost 0.50)",
        "",
        "| Month | n | Win% | Avg R | PF |",
        "|---|---:|---:|---:|---:|",
    ]
    for month in sorted({trade["month"] for trade in primary}):
        group = [trade["r"] for trade in primary if trade["month"] == month]
        stats = base.stats(group)
        lines.append(
            f"| {month} | {stats['n']} | {stats['win']} | {stats['avg']:+.3f} | {stats['pf']:.2f} |"
        )

    buys = [trade for trade in primary if trade["direction"] == "BUY"]
    sells = [trade for trade in primary if trade["direction"] == "SELL"]
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
        lines.append(f"| {name} | {stats['n']} | {stats['win']} | {stats['avg']:+.3f} | {stats['pf']:.2f} |")

    lines.extend(
        [
            "",
            "## Reading",
            "",
            "- Candidate only if cost 1.0 avg R > 0, PF > 1.2, n >= 200, >= 2.5/week.",
            "- M5 is a robustness check; a candidate must not flip negative there.",
            "- No tuning after results; forward shadow remains the promotion gate.",
            "",
        ]
    )
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
