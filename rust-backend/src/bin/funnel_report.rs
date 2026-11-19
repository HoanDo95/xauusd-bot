//! Funnel report for `data/trigger_journal.jsonl`.
//!
//! Counts the live decision funnel and the specific gate sub-reasons. Analysis
//! only: no Telegram, no signal, no state mutation.

use gold_signal_bot::trigger_journal::{TriggerEvent, TriggerJournal};
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Default, Clone)]
struct Funnel {
    lines: usize,
    observed: usize,
    setups: usize,
    triggers: usize,
    confirmation_pass: usize,
    confirmation_fail: usize,
    ai_reviews: usize,
    ai_blocked: usize,
    outcomes: usize,
    shadow_outcomes: usize,
    confirmation_failures: BTreeMap<String, usize>,
    ai_blocks: BTreeMap<String, usize>,
    decision_gate_blocks: BTreeMap<String, usize>,
    shadow_outcome_types: BTreeMap<String, usize>,
}

fn summarize(events: &[TriggerEvent]) -> Funnel {
    let mut funnel = Funnel {
        lines: events.len(),
        ..Funnel::default()
    };
    for event in events {
        match event.event.as_str() {
            "scenario_observed" => funnel.observed += 1,
            "setup_created" => funnel.setups += 1,
            "triggered" => {
                funnel.triggers += 1;
                match event.confirmation_pass {
                    Some(true) => funnel.confirmation_pass += 1,
                    Some(false) => {
                        funnel.confirmation_fail += 1;
                        let reason = event
                            .gate_subreason
                            .as_deref()
                            .or(event.confirmation_reason.as_deref())
                            .unwrap_or("unknown");
                        *funnel
                            .confirmation_failures
                            .entry(reason.to_string())
                            .or_default() += 1;
                    }
                    None => {}
                }
            }
            "ai_review" => {
                funnel.ai_reviews += 1;
                if event.ai_blocked == Some(true) {
                    funnel.ai_blocked += 1;
                    let reason = event
                        .ai_reason
                        .as_deref()
                        .unwrap_or("unknown")
                        .split(':')
                        .next()
                        .unwrap_or("unknown")
                        .to_string();
                    *funnel.ai_blocks.entry(reason).or_default() += 1;
                }
            }
            "outcome" => {
                funnel.outcomes += 1;
                if event.shadow == Some(true) {
                    funnel.shadow_outcomes += 1;
                    let kind = event
                        .shadow_outcome
                        .as_deref()
                        .or(event.outcome.as_deref())
                        .unwrap_or("unknown");
                    *funnel
                        .shadow_outcome_types
                        .entry(kind.to_string())
                        .or_default() += 1;
                }
            }
            "decision_trace" if event.valid == Some(false) => {
                let gate = event.gate.as_deref().unwrap_or("unknown");
                *funnel
                    .decision_gate_blocks
                    .entry(gate.to_string())
                    .or_default() += 1;
            }
            _ => {}
        }
    }
    funnel
}

fn format_report(path: &str, funnel: &Funnel) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Funnel Report\n");
    let _ = writeln!(out, "- Path: `{path}`");
    let _ = writeln!(out, "- Journal lines: {}", funnel.lines);
    let _ = writeln!(out, "- Scenario observed: {}", funnel.observed);
    let _ = writeln!(
        out,
        "- Setups created: {} ({:.1}% of observations)",
        funnel.setups,
        pct(funnel.setups, funnel.observed)
    );
    let _ = writeln!(
        out,
        "- Triggers: {} ({:.1}% of setups)",
        funnel.triggers,
        pct(funnel.triggers, funnel.setups)
    );
    let _ = writeln!(
        out,
        "- Confirmation: {} pass / {} fail",
        funnel.confirmation_pass, funnel.confirmation_fail
    );
    let _ = writeln!(
        out,
        "- AI reviews: {} · blocked: {}",
        funnel.ai_reviews, funnel.ai_blocked
    );
    let _ = writeln!(
        out,
        "- Outcomes: {} · shadow: {}",
        funnel.outcomes, funnel.shadow_outcomes
    );
    let _ = writeln!(out);

    out.push_str(&format_count_table(
        "Confirmation Failures",
        &funnel.confirmation_failures,
    ));
    out.push_str(&format_count_table("AI Blocks", &funnel.ai_blocks));
    out.push_str(&format_count_table(
        "Decision Gate Blocks",
        &funnel.decision_gate_blocks,
    ));
    out.push_str(&format_count_table(
        "Shadow Outcomes",
        &funnel.shadow_outcome_types,
    ));
    out
}

fn pct(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 / total as f64 * 100.0
    }
}

fn format_count_table(title: &str, counts: &BTreeMap<String, usize>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## {title}\n");
    if counts.is_empty() {
        let _ = writeln!(out, "_No data._\n");
        return out;
    }
    let _ = writeln!(out, "| Reason | Count |");
    let _ = writeln!(out, "|---|---:|");
    for (key, count) in counts {
        let _ = writeln!(out, "| {key} | {count} |");
    }
    let _ = writeln!(out);
    out
}

fn main() {
    let mut path = "data/trigger_journal.jsonl".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--path" {
            if let Some(value) = args.next() {
                path = value;
            }
        }
    }
    let journal = TriggerJournal::new(&path);
    let events = journal.load(None);
    print!("{}", format_report(&path, &summarize(&events)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn event(name: &str) -> TriggerEvent {
        let mut event = TriggerEvent::new(name);
        event.created_at = Some(Utc::now());
        event
    }

    #[test]
    fn funnel_counts_confirmation_and_shadow_reasons() {
        let mut observed = event("scenario_observed");
        observed.scenario_id = Some("bearish_breakdown".to_string());
        let mut setup = event("setup_created");
        setup.setup_id = Some("S1".to_string());
        let mut trigger = event("triggered");
        trigger.setup_id = Some("S1".to_string());
        trigger.confirmation_pass = Some(false);
        trigger.gate_subreason = Some("volume".to_string());
        let mut ai = event("ai_review");
        ai.setup_id = Some("S1".to_string());
        ai.ai_blocked = Some(true);
        ai.ai_reason = Some("AI Decision chọn STANDBY: reason".to_string());
        let mut outcome = event("outcome");
        outcome.setup_id = Some("S1".to_string());
        outcome.shadow = Some(true);
        outcome.shadow_outcome = Some("SL".to_string());
        let mut trace = event("decision_trace");
        trace.valid = Some(false);
        trace.gate = Some("session".to_string());

        let funnel = summarize(&[observed, setup, trigger, ai, outcome, trace]);
        assert_eq!(funnel.observed, 1);
        assert_eq!(funnel.setups, 1);
        assert_eq!(funnel.triggers, 1);
        assert_eq!(funnel.confirmation_fail, 1);
        assert_eq!(funnel.confirmation_failures.get("volume"), Some(&1));
        assert_eq!(funnel.ai_blocked, 1);
        assert_eq!(funnel.shadow_outcomes, 1);
        assert_eq!(funnel.shadow_outcome_types.get("SL"), Some(&1));
        assert_eq!(funnel.decision_gate_blocks.get("session"), Some(&1));
    }

    #[test]
    fn report_contains_funnel_sections() {
        let funnel = Funnel {
            lines: 6,
            observed: 1,
            setups: 1,
            triggers: 1,
            confirmation_fail: 1,
            confirmation_failures: BTreeMap::from([("volume".to_string(), 1)]),
            ..Funnel::default()
        };
        let report = format_report("test.jsonl", &funnel);
        assert!(report.contains("# Funnel Report"));
        assert!(report.contains("Confirmation Failures"));
        assert!(report.contains("volume"));
    }
}
