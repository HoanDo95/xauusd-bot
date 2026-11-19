#!/usr/bin/env python3
"""Historical walk-forward consistency for the current signal candidates.

This is a *historical* walk-forward check, not true forward OOS: the rules were
derived from the same 4.2-year sample, so the report is a robustness screen, not
proof of out-of-sample edge. True forward OOS starts when the frozen image and
config hash go live.

Candidates:
  live_all              current live-like scenario trades (all sessions)
  live_current          live-like, 13:00-23:59 VN
  live_split            live-like, 07:00-20:00 + 22:00-05:00 VN
  live_no_bounce        live-like, exclude bullish_bounce
  live_split_no_bounce  both
  wide_all              breakdown entry + 1x H4 ATR SL, all sessions
  wide_current          wide, 13:00-23:59 VN
  wide_split            wide, split session
  wide_no_bounce        wide, exclude bullish_bounce
  wide_split_no_bounce  both

Usage:
  python3 scripts/research/walk_forward_oos.py \
    --trades rust-backend/data/backtest/exness_scenarios_htf_cost002.json \
    --triggers rust-backend/data/backtest/exness_raw_triggers_v3.jsonl \
    --cache rust-backend/data/backtest/XAUUSDm_M15.jsonl \
    --out ../docs/WALK_FORWARD_OOS_2026-11-19.md
"""

import argparse
import importlib.util
import json
import math
import random
from collections import defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path

M15_MS = 15 * 60 * 1000
VN_OFFSET = timedelta(hours=7)
BASE_COST_R = 0.02
WINDOWS = [
    ("W1 2022-08-25 → 2023-08-25", "2022-08-25", "2023-08-25"),
    ("W2 2023-08-25 → 2024-08-25", "2023-08-25", "2024-08-25"),
    ("W3 2024-08-25 → 2025-08-25", "2024-08-25", "2025-08-25"),
    ("W4 2025-08-25 → 2026-11-17", "2025-08-25", "2026-11-17"),
]


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


def vn_hour(value):
    ts = parse_ts(value)
    if not ts:
        return None
    return (ts + VN_OFFSET).hour


def in_current(hour):
    return hour is not None and 13 <= hour <= 23


def in_split(hour):
    return hour is not None and (7 <= hour < 20 or hour >= 22 or hour < 5)


def in_window(date_str, start, end):
    return date_str is not None and start <= date_str < end


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


def week_key(date_str):
    ts = parse_ts(date_str)
    if not ts:
        return "unknown"
    year, week, _ = ts.isocalendar()
    return f"{year}-W{week:02d}"


def block_bootstrap(rows, iterations=3000):
    blocks = defaultdict(list)
    for date_str, value in rows:
        blocks[week_key(date_str)].append(value)
    keys = list(blocks)
    if len(keys) < 2:
        return None
    random.seed(42)
    averages = []
    for _ in range(iterations):
        sample = []
        for _ in keys:
            sample.extend(blocks[random.choice(keys)])
        averages.append(sum(sample) / len(sample))
    averages.sort()
    low = averages[int(0.025 * (len(averages) - 1))]
    high = averages[int(0.975 * (len(averages) - 1))]
    return round(low, 3), round(high, 3)


def fmt_ci(ci):
    return f"{ci[0]:+.3f} … {ci[1]:+.3f}" if ci else "n/a"


def live_rows(trades, cost_usd):
    rows = []
    for trade in trades:
        risk = trade.get("risk")
        if not risk or risk <= 0:
            continue
        value = trade["r_multiple"] - (cost_usd / risk - BASE_COST_R)
        rows.append(
            {
                "date": trade["signal_time"][:10],
                "time": trade["signal_time"],
                "hour": vn_hour(trade["signal_time"]),
                "scenario": trade.get("scenario_id", "unknown"),
                "r": value,
            }
        )
    return rows


def wide_rows(triggers, candles, ab, cost_usd):
    rows = []
    for trigger in triggers:
        direction = trigger.get("direction")
        entry = trigger.get("original_entry")
        h4_atr = trigger.get("h4_atr") or 0.0
        trigger_time = trigger.get("original_trigger_time")
        if not direction or not entry or h4_atr <= 0 or not trigger_time:
            continue
        original_ms = ab.parse_ms(trigger_time)
        if original_ms is None:
            continue
        start_index = next(
            (
                i
                for i, candle in enumerate(candles)
                if candle["timestamp"] == original_ms - M15_MS
            ),
            None,
        )
        if start_index is None:
            continue
        stop_loss = entry + h4_atr if direction == "SELL" else entry - h4_atr
        result = ab.simulate(direction, entry, stop_loss, start_index, candles, cost_usd)
        if result is None:
            continue
        rows.append(
            {
                "date": trigger_time[:10],
                "time": trigger_time,
                "hour": vn_hour(trigger_time),
                "scenario": trigger.get("scenario_id", "unknown"),
                "r": result[0],
            }
        )
    return rows


def candidate_rows(rows, session=None, no_bounce=False):
    out = []
    for row in rows:
        if session == "current" and not in_current(row["hour"]):
            continue
        if session == "split" and not in_split(row["hour"]):
            continue
        if no_bounce and row["scenario"] == "bullish_bounce":
            continue
        out.append((row["date"], row["r"]))
    return out


def render_candidate_table(rows, cost_usd):
    lines = [
        "| Candidate | n | Win% | Avg R | Total R | PF | MaxDD | Bootstrap 95% CI |",
        "|---|---:|---:|---:|---:|---:|---:|---|",
    ]
    for name, selected in rows.items():
        values = [value for _, value in selected]
        summary = stats(values)
        lines.append(
            f"| {name} | {summary['n']} | {summary['win']} | {summary['avg']:+.3f} | "
            f"{summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} | "
            f"{fmt_ci(block_bootstrap(selected))} |"
        )
    return lines


def render_window_table(rows, window_start, window_end):
    lines = [
        "| Candidate | n | Avg R | Total R | PF | MaxDD |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for name, selected in rows.items():
        filtered = [
            (date_str, value)
            for date_str, value in selected
            if in_window(date_str, window_start, window_end)
        ]
        values = [value for _, value in filtered]
        summary = stats(values)
        lines.append(
            f"| {name} | {summary['n']} | {summary['avg']:+.3f} | "
            f"{summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} |"
        )
    return lines


def render(rows, rows_cost1):
    lines = [
        "# Historical Walk-Forward Consistency — 2026-11-19",
        "",
        "> **Caveat:** this is a historical walk-forward screen on the same 4.2-year",
        "> sample used to derive the rules. It is **not** true forward OOS. True",
        "> forward OOS starts from the frozen image/config hash and runs from",
        "> 2026-11-19 onward.",
        "",
        "## Candidates",
        "",
        "- `live_*`: current live-like scenario trades (quantum_v3_htf).",
        "- `wide_*`: breakdown entry + 1×H4 ATR SL, TP1 2R, timeout 96 bars.",
        "- `current`: 13:00–23:59 VN (live inclusive).",
        "- `split`: 07:00–20:00 + 22:00–05:00 VN.",
        "- `no_bounce`: exclude `bullish_bounce`.",
        "",
        "## Pooled results — cost 0.50 USD/oz",
        "",
    ]
    lines.extend(render_candidate_table(rows, 0.5))
    lines.extend(
        [
            "",
            "## Cost stress — 1.00 USD/oz",
            "",
        ]
    )
    lines.extend(render_candidate_table(rows_cost1, 1.0))

    for label, start, end in WINDOWS:
        lines.extend(["", f"## {label}", ""])
        lines.extend(render_window_table(rows, start, end))

    lines.extend(
        [
            "",
            "## Consistency check",
            "",
            "| Candidate | Positive windows | Windows |",
            "|---|---:|---:|",
        ]
    )
    for name, selected in rows.items():
        positive = 0
        for _, start, end in WINDOWS:
            values = [
                value
                for date_str, value in selected
                if in_window(date_str, start, end)
            ]
            if values and stats(values)["avg"] > 0:
                positive += 1
        lines.append(f"| {name} | {positive} | {len(WINDOWS)} |")

    lines.extend(
        [
            "",
            "## Reading the result",
            "",
            "- A candidate that is positive in 3–4 windows and survives cost 1.0 is a",
            "  stronger promotion candidate than a single pooled average.",
            "- A candidate that is positive pooled but negative in 2+ windows is not",
            "  consistent enough to promote.",
            "- This screen only shortens the research loop; it does not replace the",
            "  forward shadow. Promotion still requires forward net expectancy > 0,",
            "  bootstrap CI low > 0, PF > 1.2, maxDD ≤ 15R, and consistency.",
            "",
        ]
    )
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--trades",
        default="rust-backend/data/backtest/exness_scenarios_htf_cost002.json",
    )
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
    data = json.load(open(args.trades))
    trades = data["variants"]["quantum_v3_htf"]["trades"]

    live = live_rows(trades, 0.5)
    wide = wide_rows(triggers, candles, ab, 0.5)
    live1 = live_rows(trades, 1.0)
    wide1 = wide_rows(triggers, candles, ab, 1.0)

    candidates = {
        "live_all": candidate_rows(live),
        "live_current": candidate_rows(live, "current"),
        "live_split": candidate_rows(live, "split"),
        "live_no_bounce": candidate_rows(live, no_bounce=True),
        "live_split_no_bounce": candidate_rows(live, "split", no_bounce=True),
        "wide_all": candidate_rows(wide),
        "wide_current": candidate_rows(wide, "current"),
        "wide_split": candidate_rows(wide, "split"),
        "wide_no_bounce": candidate_rows(wide, no_bounce=True),
        "wide_split_no_bounce": candidate_rows(wide, "split", no_bounce=True),
    }
    candidates_cost1 = {
        "live_all": candidate_rows(live1),
        "live_current": candidate_rows(live1, "current"),
        "live_split": candidate_rows(live1, "split"),
        "live_no_bounce": candidate_rows(live1, no_bounce=True),
        "live_split_no_bounce": candidate_rows(live1, "split", no_bounce=True),
        "wide_all": candidate_rows(wide1),
        "wide_current": candidate_rows(wide1, "current"),
        "wide_split": candidate_rows(wide1, "split"),
        "wide_no_bounce": candidate_rows(wide1, no_bounce=True),
        "wide_split_no_bounce": candidate_rows(wide1, "split", no_bounce=True),
    }

    report = render(candidates, candidates_cost1)
    print(report)
    if args.out:
        Path(args.out).write_text(report)


if __name__ == "__main__":
    main()
