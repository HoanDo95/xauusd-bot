//! Markdown report for `data/news_reactions.json`.
//!
//! Usage:
//!   cargo run --bin news_report -- --path data/news_reactions.json
//!
//! Analysis-only: never sends Telegram, changes a signal or touches the
//! forward-test trigger journal.

use gold_signal_bot::news_impact::{
    summarize, NewsImpactRecord, NewsImpactStore, NewsImpactSummary,
};
use std::fmt::Write;

fn main() {
    let mut path = "data/news_reactions.json".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--path" {
            if let Some(value) = args.next() {
                path = value;
            }
        }
    }
    let store = NewsImpactStore::new(&path);
    let records = store.load();
    let summary = summarize(&records);
    print!("{}", format_report(&path, &records, &summary));
}

fn pct(value: Option<f64>) -> String {
    value
        .map(|value| format!("{:.1}%", value * 100.0))
        .unwrap_or_else(|| "n/a".to_string())
}

fn points(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_report(path: &str, records: &[NewsImpactRecord], summary: &NewsImpactSummary) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# News Impact & Reaction Report\n");
    let _ = writeln!(out, "- Path: `{path}`");
    let _ = writeln!(out, "- Records: {}", summary.total_events);
    let _ = writeln!(
        out,
        "- Measured (reaction known): {} · UP {} · DOWN {} · FLAT {} · UNKNOWN {}",
        summary.measured_events, summary.up, summary.down, summary.flat, summary.unknown
    );
    let _ = writeln!(
        out,
        "- Avg |move|: +15m {} pts · +60m {} pts · +240m {} pts",
        points(summary.avg_abs_move_15m),
        points(summary.avg_abs_move_60m),
        points(summary.avg_abs_move_240m)
    );
    let _ = writeln!(
        out,
        "- Deterministic conditional lean: {}/{} correct ({})",
        summary.conditional_correct,
        summary.conditional_total,
        pct(summary.conditional_hit_rate)
    );
    let _ = writeln!(
        out,
        "- AI lean: {}/{} correct ({})",
        summary.ai_correct,
        summary.ai_total,
        pct(summary.ai_hit_rate)
    );
    let _ = writeln!(
        out,
        "- Majority DOWN share (directional only): {}",
        pct(summary.majority_down_share)
    );
    let _ = writeln!(out);

    if summary.total_events == 0 {
        let _ = writeln!(
            out,
            "> Chưa có record nào. Worker chạy mỗi 5 phút và cần ít nhất một sự kiện High/Medium USD đã qua.\n"
        );
        return out;
    }

    let _ = writeln!(out, "## By Event Class\n");
    let _ = writeln!(
        out,
        "| Class | Count | UP | DOWN | FLAT | UNKNOWN | Avg |60m| | Conditional hit | AI hit |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for bucket in &summary.by_class {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            bucket.event_class.as_str(),
            bucket.count,
            bucket.up,
            bucket.down,
            bucket.flat,
            bucket.unknown,
            points(bucket.avg_abs_move_60m),
            pct(bucket.conditional_hit_rate),
            pct(bucket.ai_hit_rate)
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Recent Records\n");
    let _ = writeln!(
        out,
        "| Time (UTC) | Event | Actual / Forecast | Lean | Reaction | +60m | AI |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---:|---|");
    for record in records.iter().rev().take(20) {
        let ai = record
            .ai
            .as_ref()
            .map(|view| {
                format!(
                    "{} {}% ({})",
                    view.gold_lean.as_str(),
                    view.confidence,
                    view.model
                )
            })
            .unwrap_or_else(|| "n/a".to_string());
        let actual = record.actual.as_deref().unwrap_or("n/a");
        let forecast = record.forecast.as_deref().unwrap_or("n/a");
        let _ = writeln!(
            out,
            "| {} | {} | {} / {} | {} | {} | {} | {} |",
            record.scheduled_time.format("%m-%d %H:%M"),
            record.title.replace('|', "/"),
            actual,
            forecast,
            record.conditional_lean.as_str(),
            record.reaction.as_str(),
            points(record.return_60m),
            ai
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Methodology\n");
    let _ = writeln!(
        out,
        "- Reaction baseline = last closed M15 candle at/before the event time."
    );
    let _ = writeln!(
        out,
        "- `UP`/`DOWN` requires a move of at least `0.3 × H4 ATR` (fallback 2.0 points) at +60m, else +15m."
    );
    let _ = writeln!(
        out,
        "- Conditional hit rate only counts records with a directional lean and a directional reaction (UP/DOWN); FLAT/UNKNOWN are excluded."
    );
    let _ = writeln!(
        out,
        "- AI hit rate uses the same directional-only denominator. AI runs only after `actual` is present, High USD + High relevance, capped per day."
    );
    let _ = writeln!(
        out,
        "- This report is measurement-only. It must not be used to change the sniper threshold during the forward test."
    );
    out
}
