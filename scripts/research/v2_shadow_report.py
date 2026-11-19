#!/usr/bin/env python3
"""Forward comparison: strategy v2 shadow vs live confirmed triggers.

Reads the production trigger journal, joins each `strategy_v2_shadow` and
`triggered` entry with its `outcome` row, and reports gross/net R, PF, max DD
and a weekly block bootstrap. Research only; the journal is never modified.

Usage:
  python3 scripts/research/v2_shadow_report.py \
    --journal /home/jason/xauusd-bot/data/trigger_journal.jsonl \
    --out docs/V2_SHADOW_FORWARD.md
"""

import argparse
import json
import math
import random
import statistics
from collections import defaultdict
from datetime import datetime, timezone


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
    # Rust emits nanosecond precision; Python < 3.11 accepts at most 6 digits.
    if "." in text:
        head, rest = text.split(".", 1)
        fraction, sep, tail = rest.partition("+")
        if not sep:
            fraction, sep, tail = rest.partition("-")
        text = f"{head}.{fraction[:6]}{sep}{tail}"
    return datetime.fromisoformat(text)


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
    created = parse_ts(trigger.get("created_at"))
    if not created:
        return "unknown"
    year, week, _ = created.isocalendar()
    return f"{year}-W{week:02d}"


def block_bootstrap(rows, iterations=3000):
    blocks = defaultdict(list)
    for trigger, r in rows:
        blocks[week_key(trigger)].append(r)
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


def permutation_p(rows_a, rows_b, iterations=2000):
    values = [r for _, r in rows_a] + [r for _, r in rows_b]
    if len(rows_a) < 10 or len(rows_b) < 10:
        return None
    observed = abs(
        sum(r for _, r in rows_a) / len(rows_a)
        - sum(r for _, r in rows_b) / len(rows_b)
    )
    random.seed(7)
    count = 0
    for _ in range(iterations):
        random.shuffle(values)
        a = values[: len(rows_a)]
        b = values[len(rows_a) :]
        diff = abs(sum(a) / len(a) - sum(b) / len(b))
        if diff >= observed:
            count += 1
    return count / iterations


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


def collect(rows, cost_usd, since=None, session_filter=None):
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
        if event in ("strategy_v2_shadow", "triggered"):
            if session_filter and row.get("session") != session_filter:
                continue
            triggers[setup_id] = row
        elif event == "outcome":
            outcomes[setup_id] = row

    v2_rows = []
    live_rows = []
    pending = {"v2": 0, "live": 0}
    for setup_id, trigger in triggers.items():
        outcome = outcomes.get(setup_id)
        if trigger.get("event") == "strategy_v2_shadow":
            if not outcome:
                pending["v2"] += 1
                continue
            value = net_r(trigger, outcome, cost_usd)
            if value is not None:
                v2_rows.append((trigger, value))
        else:
            # Only actual live signals: a signal id is attached after AI/risk
            # approval. Blocked triggers stay in the research funnel instead.
            if not trigger.get("signal_id"):
                continue
            if not outcome:
                pending["live"] += 1
                continue
            value = net_r(trigger, outcome, cost_usd)
            if value is not None:
                live_rows.append((trigger, value))
    return v2_rows, live_rows, pending


def render(v2_rows, live_rows, pending, cost_usd):
    v2_values = [r for _, r in v2_rows]
    live_values = [r for _, r in live_rows]
    v2 = stats(v2_values)
    live = stats(live_values)
    v2_ci = block_bootstrap(v2_rows)
    live_ci = block_bootstrap(live_rows)
    p_value = permutation_p(v2_rows, live_rows)

    lines = [
        "# Strategy v2 shadow — forward report",
        "",
        f"- Cost: **{cost_usd:.2f} USD/oz** (subtracted from gross R using journal risk)",
        f"- Journal triggers: v2 shadow **{v2['n']}** completed / {pending['v2']} pending; "
        f"live confirmed **{live['n']}** completed / {pending['live']} pending",
        "",
        "| Group | n | Win% | Avg R | Total R | PF | MaxDD | Bootstrap 95% CI |",
        "|---|---:|---:|---:|---:|---:|---:|---|",
    ]

    def fmt_ci(ci):
        return f"{ci[0]:+.3f} … {ci[1]:+.3f}" if ci else "n/a"

    lines.append(
        f"| v2 shadow | {v2['n']} | {v2['win']} | {v2['avg']:+.3f} | {v2['total']:+.1f} | "
        f"{v2['pf']:.2f} | {v2['dd']:.1f} | {fmt_ci(v2_ci)} |"
    )
    lines.append(
        f"| live confirmed | {live['n']} | {live['win']} | {live['avg']:+.3f} | {live['total']:+.1f} | "
        f"{live['pf']:.2f} | {live['dd']:.1f} | {fmt_ci(live_ci)} |"
    )
    lines.append("")
    lines.append("## v2 shadow by session")
    lines.append("")
    lines.append("| Session | n | Win% | Avg R | Total R | PF |")
    lines.append("|---|---:|---:|---:|---:|---:|")
    by_session = defaultdict(list)
    for trigger, r in v2_rows:
        by_session[trigger.get("session") or "unknown"].append(r)
    for session in sorted(by_session):
        values = by_session[session]
        row = stats(values)
        lines.append(
            f"| {session} | {row['n']} | {row['win']} | {row['avg']:+.3f} | "
            f"{row['total']:+.1f} | {row['pf']:.2f} |"
        )
    lines.append("")
    if p_value is None:
        lines.append("- v2 vs live permutation: not enough completed samples yet.")
    else:
        lines.append(f"- v2 vs live permutation p = **{p_value:.3f}** (two-sided).")
    lines.append(
        "- Promotion gate: v2 net expectancy > 0, bootstrap CI low > 0, PF > 1.2, "
        "maxDD ≤ 15R, consistent across periods/ATR buckets."
    )
    lines.append("")
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
    parser.add_argument(
        "--session",
        default="",
        help="Filter both groups by journal session label (e.g. IN).",
    )
    args = parser.parse_args()

    since = parse_ts(args.since) if args.since else None
    rows = load_journal(args.journal)
    v2_rows, live_rows, pending = collect(
        rows, args.cost_usd, since, args.session or None
    )
    report = render(v2_rows, live_rows, pending, args.cost_usd)
    print(report)
    if args.out:
        with open(args.out, "w") as handle:
            handle.write(report)


if __name__ == "__main__":
    main()
