#!/usr/bin/env python3
"""Engine signals with an executable market entry (research only).

The engine backtest fills at the scenario level (touch/limit). This script takes
the same original triggers and enters at the trigger candle close, keeping the
original SL/TP1/TP2, to test whether the signal is tradeable at market.

Usage:
  python3 scripts/research/scalp_engine_market_entry_ab.py \
    --triggers rust-backend/data/backtest/exness_raw_triggers_v3.jsonl \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/SCALP_ENGINE_MARKET_ENTRY_AB_2026-11-20.md
"""

import argparse
import importlib.util
import math
from datetime import datetime, timezone
from pathlib import Path

M15_MS = 15 * 60 * 1000
COST_USD = 0.5
COST_USD_STRESS = 1.0
TIMEOUT_BARS = 96


def load_module():
    path = Path(__file__).with_name("scalp_m15_standalone_ab.py")
    spec = importlib.util.spec_from_file_location("scalp_m15_standalone_ab", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def parse_ms(value):
    text = value.replace("Z", "+00:00")
    return int(datetime.fromisoformat(text).timestamp() * 1000)


def session_of(ts_ms):
    hour = datetime.fromtimestamp(ts_ms / 1000, tz=timezone.utc).hour
    if 0 <= hour < 7:
        return "Asia 00-07"
    if 7 <= hour < 8:
        return "Pre-London 07-08"
    if 8 <= hour < 13:
        return "London 08-13"
    if 13 <= hour < 16:
        return "Overlap 13-16"
    if 16 <= hour < 22:
        return "Late NY 16-22"
    return "Late 22-24"


def simulate_fixed(bars, direction, entry, stop_loss, tp1, tp2, start_index, cost_usd):
    risk = abs(entry - stop_loss)
    if risk <= 0:
        return None
    tp1_r = (
        (entry - tp1) / risk if direction == "SELL" else (tp1 - entry) / risk
    )
    tp2_r = (
        (entry - tp2) / risk if direction == "SELL" else (tp2 - entry) / risk
    )
    tp1_hit = False
    for offset in range(1, TIMEOUT_BARS + 1):
        index = start_index + offset
        if index >= bars["n"]:
            break
        high, low = bars["h"][index], bars["l"][index]
        if not tp1_hit:
            if direction == "SELL":
                if high >= stop_loss:
                    return -1.0 - cost_usd / risk, offset
                if low <= tp1:
                    tp1_hit = True
            else:
                if low <= stop_loss:
                    return -1.0 - cost_usd / risk, offset
                if high >= tp1:
                    tp1_hit = True
        else:
            if direction == "SELL" and high >= entry:
                return 0.5 * tp1_r - cost_usd / risk, offset
            if direction == "BUY" and low <= entry:
                return 0.5 * tp1_r - cost_usd / risk, offset
        if tp1_hit:
            if direction == "SELL" and low <= tp2:
                return 0.5 * (tp1_r + tp2_r) - cost_usd / risk, offset
            if direction == "BUY" and high >= tp2:
                return 0.5 * (tp1_r + tp2_r) - cost_usd / risk, offset
    index = min(start_index + TIMEOUT_BARS, bars["n"] - 1)
    close_r = (
        (entry - bars["c"][index]) / risk
        if direction == "SELL"
        else (bars["c"][index] - entry) / risk
    )
    if tp1_hit:
        return 0.5 * tp1_r + 0.5 * close_r - cost_usd / risk, TIMEOUT_BARS
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--triggers", default="rust-backend/data/backtest/exness_raw_triggers_v3.jsonl")
    parser.add_argument("--cache", default="rust-backend/data/backtest/XAUUSDm_M15.jsonl")
    parser.add_argument("--out", default="")
    args = parser.parse_args()

    base = load_module()
    bars = base.load_bars(args.cache)
    index_by_ts = {ts: i for i, ts in enumerate(bars["ts"])}
    rows = [
        __import__("json").loads(line)
        for line in Path(args.triggers).read_text().splitlines()
        if line.strip()
    ]
    rows.sort(key=lambda row: row["original_trigger_time"])
    weeks = (bars["ts"][-1] - bars["ts"][0]) / (7 * 24 * 60 * 60 * 1000)

    candidates = []
    missing = 0
    for row in rows:
        if row.get("scenario_id") == "bullish_bounce":
            continue
        direction = row.get("direction")
        if direction not in ("BUY", "SELL"):
            continue
        trigger_ms = parse_ms(row["original_trigger_time"])
        index = index_by_ts.get(trigger_ms - M15_MS)
        if index is None:
            missing += 1
            continue
        entry = bars["c"][index]
        stop_loss = row.get("original_stop_loss")
        tp1 = row.get("original_take_profit_1")
        tp2 = row.get("original_take_profit_2")
        if not stop_loss or not tp1 or not tp2:
            continue
        candidates.append(
            {
                "ts": trigger_ms,
                "index": index,
                "direction": direction,
                "entry": entry,
                "stop_loss": stop_loss,
                "tp1": tp1,
                "tp2": tp2,
                "session": session_of(trigger_ms),
                "scenario": row.get("scenario_id"),
                "confidence": row.get("confidence") or 0,
            }
        )

    def run_sequential(items):
        trades = []
        next_free = 0
        for item in items:
            if item["ts"] < next_free:
                continue
            result = simulate_fixed(
                bars,
                item["direction"],
                item["entry"],
                item["stop_loss"],
                item["tp1"],
                item["tp2"],
                item["index"],
                COST_USD,
            )
            result1 = simulate_fixed(
                bars,
                item["direction"],
                item["entry"],
                item["stop_loss"],
                item["tp1"],
                item["tp2"],
                item["index"],
                COST_USD_STRESS,
            )
            if result is None or result1 is None:
                continue
            trades.append(
                {
                    **item,
                    "r": result[0],
                    "r1": result1[0],
                    "bars": result[1],
                }
            )
            next_free = item["ts"] + result[1] * M15_MS
        return trades

    def run_all(items):
        trades = []
        for item in items:
            result = simulate_fixed(
                bars,
                item["direction"],
                item["entry"],
                item["stop_loss"],
                item["tp1"],
                item["tp2"],
                item["index"],
                COST_USD,
            )
            result1 = simulate_fixed(
                bars,
                item["direction"],
                item["entry"],
                item["stop_loss"],
                item["tp1"],
                item["tp2"],
                item["index"],
                COST_USD_STRESS,
            )
            if result is None or result1 is None:
                continue
            trades.append({**item, "r": result[0], "r1": result1[0], "bars": result[1]})
        return trades

    sequential = run_sequential(candidates)
    overlapping = run_all(candidates)

    lines = [
        "# Engine signals — executable market entry",
        "",
        "> Enters at the trigger candle close with the original SL/TP levels.",
        "> Research only; no live change.",
        "",
        f"- Data: **{bars['n']}** M15 bars; triggers **{len(rows)}**, usable **{len(candidates)}**, "
        f"missing {missing}",
        f"- Weeks: **{weeks:.1f}**",
        "- PRIMARY only (excludes bullish_bounce). Cost 0.50 / 1.00 USD/oz.",
        "",
        "## Sequential (one trade at a time)",
        "",
        "| Session | n | /week | Win% | Avg R 0.50 | PF | Avg R 1.00 | PF |",
        "|---|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for session in [
        "All",
        "Asia 00-07",
        "Pre-London 07-08",
        "London 08-13",
        "Overlap 13-16",
        "Late NY 16-22",
        "Late 22-24",
    ]:
        group = sequential if session == "All" else [t for t in sequential if t["session"] == session]
        base_stats = stats([t["r"] for t in group])
        stress_stats = stats([t["r1"] for t in group])
        lines.append(
            f"| {session} | {base_stats['n']} | {base_stats['n']/weeks:.2f} | {base_stats['win']} | "
            f"{base_stats['avg']:+.3f} | {base_stats['pf']:.2f} | {stress_stats['avg']:+.3f} | "
            f"{stress_stats['pf']:.2f} |"
        )

    lines.extend(
        [
            "",
            "## Overlapping (all signals, engine-style)",
            "",
            "| Session | n | /week | Win% | Avg R 0.50 | PF | Avg R 1.00 | PF |",
            "|---|---:|---:|---:|---:|---:|---:|---:|",
        ]
    )
    for session in ["All", "London 08-13", "Late NY 16-22"]:
        group = overlapping if session == "All" else [t for t in overlapping if t["session"] == session]
        base_stats = stats([t["r"] for t in group])
        stress_stats = stats([t["r1"] for t in group])
        lines.append(
            f"| {session} | {base_stats['n']} | {base_stats['n']/weeks:.2f} | {base_stats['win']} | "
            f"{base_stats['avg']:+.3f} | {base_stats['pf']:.2f} | {stress_stats['avg']:+.3f} | "
            f"{stress_stats['pf']:.2f} |"
        )

    lines.extend(
        [
            "",
            "## Split session (07-20 + 22-05 UTC)",
            "",
        ]
    )
    split = [
        t
        for t in sequential
        if t["session"] in ("Pre-London 07-08", "London 08-13", "Overlap 13-16", "Late NY 16-22", "Late 22-24")
    ]
    base_stats = stats([t["r"] for t in split])
    stress_stats = stats([t["r1"] for t in split])
    lines.extend(
        [
            "| n | /week | Win% | Avg R 0.50 | PF | Avg R 1.00 | PF | MaxDD 0.50 |",
            "|---:|---:|---:|---:|---:|---:|---:|---:|",
            f"| {base_stats['n']} | {base_stats['n']/weeks:.2f} | {base_stats['win']} | "
            f"{base_stats['avg']:+.3f} | {base_stats['pf']:.2f} | {stress_stats['avg']:+.3f} | "
            f"{stress_stats['pf']:.2f} | {base_stats['dd']:.1f} |",
            "",
            "## Reading",
            "",
            "- This is the same trigger set as the engine backtest, but with a market",
            "  entry instead of a limit fill at the level.",
            "- If it is negative while the published limit-fill numbers are positive, the",
            "  engine edge is also a fill artifact.",
            "",
        ]
    )
    report = "\n".join(lines)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
