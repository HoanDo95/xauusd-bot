//! Markdown report for `data/trigger_journal.jsonl`.
//!
//! Usage:
//!   cargo run --bin trigger_report -- --path data/trigger_journal.jsonl
//!
//! The report is analysis-only. It never sends Telegram or changes a signal.

use gold_signal_bot::trigger_journal::{summarize, BucketSummary, TriggerJournal, TriggerSummary};
use std::fmt::Write;

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
    let summary = summarize(&events);
    print!("{}", format_report(&path, events.len(), &summary));
}

fn format_report(path: &str, lines: usize, summary: &TriggerSummary) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Trigger Journal Report\n");
    let _ = writeln!(out, "- Path: `{path}`");
    let _ = writeln!(out, "- Journal lines: {lines}");
    let _ = writeln!(out, "- Schema: {}", summary.schema);
    let _ = writeln!(
        out,
        "- Watching setups: {} · triggered: {} · trigger rate: {:.1}%",
        summary.watching_setups,
        summary.watching_triggered,
        summary.watching_trigger_rate * 100.0
    );
    let _ = writeln!(out, "- Scanner-only triggers: {}", summary.scanner_triggers);
    let _ = writeln!(
        out,
        "- Outcomes: {} (TP1 {}, SL {}, TIMEOUT {}, DATA_GAP {})",
        summary.outcomes, summary.wins, summary.losses, summary.timeouts, summary.data_gaps
    );
    let _ = writeln!(out, "- Win rate: {:.1}%", summary.win_rate * 100.0);
    let _ = writeln!(out, "- Avg R / expectancy: {:+.2}R", summary.avg_r);
    let _ = writeln!(out, "- Profit factor: {}", format_pf(summary.profit_factor));
    let _ = writeln!(out, "- Max drawdown: {:.2}R", summary.max_drawdown_r);
    let _ = writeln!(out);

    if summary.outcomes == 0 {
        let _ = writeln!(
            out,
            "> Chưa có outcome nào. Chạy bot qua ít nhất một phiên (hoặc import journal) rồi chạy lại report.\n"
        );
    }

    out.push_str(&format_bucket_table("By Scenario", &summary.by_scenario));
    out.push_str(&format_bucket_table("By Regime", &summary.by_regime));
    out.push_str(&format_bucket_table("By Session", &summary.by_session));
    out.push_str(&format_bucket_table(
        "By AI Confidence",
        &summary.by_ai_confidence,
    ));
    out.push_str(&format_bucket_table(
        "By Confirmation",
        &summary.by_confirmation,
    ));

    let _ = writeln!(out, "## Methodology\n");
    let _ = writeln!(
        out,
        "- **Trigger hit rate** = watching setups whose frozen trigger was hit / watching setups created."
    );
    let _ = writeln!(
        out,
        "- **Outcome** is hypothetical TP1/SL/TIMEOUT from the candles after the confirming M15 close; both TP and SL in the same candle count as SL first (conservative)."
    );
    let _ = writeln!(
        out,
        "- **R multiple** uses the scenario risk (|entry − stop|). TP1 is positive reward/risk; SL is −1R; TIMEOUT is the close mark-to-market."
    );
    let _ = writeln!(
        out,
        "- **MFE/MAE** are favorable/adverse excursions in R at 1/4/8/24 M15 bars. Missing checkpoints mean the outcome closed earlier."
    );
    let _ = writeln!(
        out,
        "- **AI confidence buckets** use the `ai_review` event; `NO_AI` means no review was recorded (provider unavailable or no review)."
    );
    let _ = writeln!(
        out,
        "- Use this report to prune negative buckets and calibrate AI confidence; do not tune parameters mid-test."
    );
    out
}

fn format_bucket_table(title: &str, buckets: &[BucketSummary]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## {title}\n");
    if buckets.is_empty() {
        let _ = writeln!(out, "_No data._\n");
        return out;
    }
    let _ = writeln!(out, "| Key | N | Win% | Avg R | PF | Expectancy |");
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|");
    for bucket in buckets {
        let _ = writeln!(
            out,
            "| {} | {} | {:.1}% | {:+.2}R | {} | {:+.2}R |",
            bucket.key,
            bucket.outcomes,
            bucket.win_rate * 100.0,
            bucket.avg_r,
            format_pf(bucket.profit_factor),
            bucket.expectancy
        );
    }
    let _ = writeln!(out);
    out
}

fn format_pf(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_contains_core_metrics_and_methodology() {
        let summary = TriggerSummary {
            schema: 1,
            watching_setups: 10,
            watching_triggered: 6,
            scanner_triggers: 1,
            watching_trigger_rate: 0.6,
            outcomes: 6,
            wins: 3,
            losses: 2,
            timeouts: 1,
            data_gaps: 0,
            win_rate: 0.5,
            avg_r: 0.33,
            expectancy: 0.33,
            profit_factor: Some(1.5),
            max_drawdown_r: 2.0,
            by_scenario: vec![BucketSummary {
                key: "bearish_breakdown".to_string(),
                outcomes: 6,
                wins: 3,
                losses: 2,
                timeouts: 1,
                win_rate: 0.5,
                avg_r: 0.33,
                expectancy: 0.33,
                profit_factor: Some(1.5),
            }],
            ..TriggerSummary::default()
        };
        let report = format_report("data/trigger_journal.jsonl", 42, &summary);
        assert!(report.contains("# Trigger Journal Report"));
        assert!(report.contains("Watching setups: 10"));
        assert!(report.contains("Win rate: 50.0%"));
        assert!(report.contains("Profit factor: 1.50"));
        assert!(report.contains("By Scenario"));
        assert!(report.contains("bearish_breakdown"));
        assert!(report.contains("Methodology"));
    }
}
