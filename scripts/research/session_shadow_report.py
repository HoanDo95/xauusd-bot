#!/usr/bin/env python3
"""Forward report for the session shadow (confirmed scenario, all sessions).

Reads the production trigger journal, joins `session_shadow` triggers with their
`outcome` rows, and compares session windows after cost. Research only; the
journal is never modified.

Usage:
  python3 scripts/research/session_shadow_report.py \
    --journal /home/jason/xauusd-bot/data/trigger_journal.jsonl \
    --out docs/SESSION_SHADOW_2026-11-19.md
"""

import argparse
import json
import math
import random
from collections import defaultdict
from datetime import datetime, timedelta, timezone

M15_MS = 15 * 60 * 1000
VN_OFFSET = timedelta(hours=7)


def load_journal(path):
    rows = []
    with open(path) as handle:
        for line in handle:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


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
    parsed = datetime.fromisoformat(text)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed


def trigger_time(trigger):
    candle = trigger.get("candle_timestamp")
    if candle is not None:
        # Candle timestamp is the open; signals are evaluated after the close.
        return datetime.fromtimestamp((candle + M15_MS) / 1000.0, tz=timezone.utc)
    return parse_ts(trigger.get("created_at"))


def vn_hour(trigger):
    ts = trigger_time(trigger)
    if ts is None:
        return None
    return (ts + VN_OFFSET).hour


def in_window(hour, name):
    if hour is None:
        return False
    if name == "current_13_23":
        return 13 <= hour <= 23
    if name == "current_13_22":
        return 13 <= hour < 23
    if name == "split_07_20_22_05":
        return 7 <= hour < 20 or hour >= 22 or hour < 5
    if name == "full_24h":
        return True
    if name == "asia_07_14":
        return 7 <= hour < 14
    if name == "pre_london_14_15":
        return 14 <= hour < 15
    if name == "london_15_20":
        return 15 <= hour < 20
    if name == "overlap_20_23":
        return 20 <= hour < 23
    if name == "late_ny_23_05":
        return hour >= 23 or hour < 5
    if name == "late_05_07":
        return 5 <= hour < 7
    return False


SCENARIO_ORDER = [
    "bearish_breakdown",
    "bullish_breakout",
    "bullish_bounce",
    "bearish_rejection",
]

WINDOWS = [
    ("current_13_23", "Current 13:00–23:59 VN (live inclusive)"),
    ("current_13_22", "Current 13:00–22:59 VN (backtest bucket)"),
    ("split_07_20_22_05", "Split 07:00–20:00 + 22:00–05:00 VN"),
    ("full_24h", "Full 24h"),
    ("asia_07_14", "Asia 07:00–14:00 VN"),
    ("pre_london_14_15", "Pre-London 14:00–15:00 VN"),
    ("london_15_20", "London 15:00–20:00 VN"),
    ("overlap_20_23", "London/NY overlap 20:00–23:00 VN"),
    ("late_ny_23_05", "Late NY 23:00–05:00 VN"),
    ("late_05_07", "Late 05:00–07:00 VN"),
]


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


def week_key(trigger):
    ts = trigger_time(trigger)
    if not ts:
        return "unknown"
    year, week, _ = ts.isocalendar()
    return f"{year}-W{week:02d}"


def block_bootstrap(rows, iterations=3000):
    blocks = defaultdict(list)
    for trigger, value in rows:
        blocks[week_key(trigger)].append(value)
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


def net_r(trigger, outcome, cost_usd):
    gross = outcome.get("r_multiple")
    if gross is None:
        gross = outcome.get("shadow_r")
    if gross is None:
        return None
    risk = trigger.get("risk_points")
    if risk and risk > 0:
        return gross - cost_usd / risk
    return gross


def collect(rows, cost_usd, since=None):
    triggers = {}
    outcomes = {}
    for row in rows:
        event = row.get("event")
        setup_id = row.get("setup_id")
        if not setup_id:
            continue
        created = parse_ts(row.get("created_at"))
        if since and created and created < since:
            continue
        if event in ("session_shadow", "triggered"):
            triggers[setup_id] = row
        elif event == "outcome":
            outcomes[setup_id] = row

    shadow_rows = []
    live_rows = []
    pending = {"shadow": 0, "live": 0}
    for setup_id, trigger in triggers.items():
        outcome = outcomes.get(setup_id)
        if trigger.get("event") == "session_shadow":
            if not outcome:
                pending["shadow"] += 1
                continue
            value = net_r(trigger, outcome, cost_usd)
            if value is not None:
                shadow_rows.append((trigger, value))
        else:
            if not trigger.get("signal_id"):
                continue
            if not outcome:
                pending["live"] += 1
                continue
            value = net_r(trigger, outcome, cost_usd)
            if value is not None:
                live_rows.append((trigger, value))
    return shadow_rows, live_rows, pending


def fmt_ci(ci):
    return f"{ci[0]:+.3f} … {ci[1]:+.3f}" if ci else "n/a"


def scenario_name(trigger):
    return trigger.get("scenario_id") or "unknown"


def scenario_sort_key(name):
    try:
        return (0, SCENARIO_ORDER.index(name))
    except ValueError:
        return (1, name)


def scenario_groups(rows):
    groups = defaultdict(list)
    for trigger, value in rows:
        groups[scenario_name(trigger)].append((trigger, value))
    return groups


def window_cell(rows, window):
    values = [
        value
        for trigger, value in rows
        if in_window(vn_hour(trigger), window)
    ]
    if not values:
        return "—"
    summary = stats(values)
    return f"{summary['avg']:+.3f}R · PF {summary['pf']:.2f} · n={summary['n']}"


def render(shadow_rows, live_rows, pending, cost_usd):
    lines = [
        "# Session Shadow — forward report",
        "",
        f"- Cost: **{cost_usd:.2f} USD/oz** (subtracted from gross R using journal risk)",
        f"- Session shadow: **{len(shadow_rows)}** completed / {pending['shadow']} pending",
        f"- Live confirmed reference: **{len(live_rows)}** completed / {pending['live']} pending",
        "",
        "## Session windows",
        "",
        "| Window | n | Win% | Avg R | Total R | PF | MaxDD | Bootstrap 95% CI |",
        "|---|---:|---:|---:|---:|---:|---:|---|",
    ]
    for key, label in WINDOWS:
        rows = [(trigger, value) for trigger, value in shadow_rows if in_window(vn_hour(trigger), key)]
        values = [value for _, value in rows]
        summary = stats(values)
        ci = block_bootstrap(rows)
        lines.append(
            f"| {label} | {summary['n']} | {summary['win']} | {summary['avg']:+.3f} | "
            f"{summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} | {fmt_ci(ci)} |"
        )

    groups = scenario_groups(shadow_rows)
    lines.extend(
        [
            "",
            "## By scenario",
            "",
            "| Scenario | n | Win% | Avg R | Total R | PF | MaxDD | Bootstrap 95% CI |",
            "|---|---:|---:|---:|---:|---:|---:|---|",
        ]
    )
    for name in sorted(groups, key=scenario_sort_key):
        rows = groups[name]
        values = [value for _, value in rows]
        summary = stats(values)
        lines.append(
            f"| {name} | {summary['n']} | {summary['win']} | {summary['avg']:+.3f} | "
            f"{summary['total']:+.1f} | {summary['pf']:.2f} | {summary['dd']:.1f} | "
            f"{fmt_ci(block_bootstrap(rows))} |"
        )

    lines.extend(
        [
            "",
            "## Scenario × key windows",
            "",
            "| Scenario | Current 13–23 | Split 07–20 + 22–05 | Full 24h |",
            "|---|---|---|---|",
        ]
    )
    for name in sorted(groups, key=scenario_sort_key):
        rows = groups[name]
        lines.append(
            f"| {name} | {window_cell(rows, 'current_13_23')} | "
            f"{window_cell(rows, 'split_07_20_22_05')} | "
            f"{window_cell(rows, 'full_24h')} |"
        )

    live_values = [value for _, value in live_rows]
    live = stats(live_values)
    lines.extend(
        [
            "",
            "## Live confirmed reference",
            "",
            "| Group | n | Win% | Avg R | Total R | PF | MaxDD | Bootstrap 95% CI |",
            "|---|---:|---:|---:|---:|---:|---:|---|",
            f"| live confirmed | {live['n']} | {live['win']} | {live['avg']:+.3f} | "
            f"{live['total']:+.1f} | {live['pf']:.2f} | {live['dd']:.1f} | "
            f"{fmt_ci(block_bootstrap(live_rows))} |",
            "",
            "## Notes",
            "",
            "- `current_13_23` matches the live code's inclusive `13..=23` hour check.",
            "- `current_13_22` matches the backtest `13:00–22:59` bucket; the 23:00–23:59",
            "  hour is the only difference.",
            "- The shadow records the confirmed scenario **before AI/news/spread/quota/session**",
            "  gates, so it measures session edge, not the full live pipeline.",
            "- The scenario breakdown is the evidence gate for filtering `bullish_bounce` or",
            "  changing the session window; do not filter live without forward evidence.",
            "- Windows overlap by design; compare them as research cuts, not independent samples.",
            "- Promotion requires forward net expectancy > 0, bootstrap CI low > 0, PF > 1.2,",
            "  maxDD ≤ 15R, and consistency across periods/ATR buckets.",
            "",
        ]
    )
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--journal",
        default="/home/jason/xauusd-bot/data/trigger_journal.jsonl",
    )
    parser.add_argument("--out", default="")
    parser.add_argument("--cost-usd", type=float, default=0.5)
    parser.add_argument("--since", default="")
    args = parser.parse_args()

    since = parse_ts(args.since) if args.since else None
    rows = load_journal(args.journal)
    shadow_rows, live_rows, pending = collect(rows, args.cost_usd, since)
    report = render(shadow_rows, live_rows, pending, args.cost_usd)
    print(report)
    if args.out:
        with open(args.out, "w") as handle:
            handle.write(report)


if __name__ == "__main__":
    main()
