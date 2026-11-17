//! News impact & reaction journal (advisory, measurement-only).
//!
//! This module answers two questions without touching the signal engine:
//! 1. What kind of event is it, and what is the conditional gold lean if
//!    `actual` beats/misses `forecast`?
//! 2. What did gold actually do at +15m / +60m / +240m after the release?
//!
//! The records live in `data/news_reactions.json`; they are intentionally
//! separate from `trigger_journal.jsonl` (schema v1) and `bot_state.json`.
//! Nothing here may block/approve a signal, change the kill switch, change the
//! 60/30/10 score, or alter the sniper threshold.

use crate::models::{Candle, Timeframe};
use crate::news_filter::{EconomicEvent, ImpactLevel};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub const NEWS_IMPACT_SCHEMA: u8 = 1;
pub const MAX_NEWS_IMPACT_RECORDS: usize = 500;
/// Reaction label threshold: 0.3 × H4 ATR (fallback 2.0 points).
pub const REACTION_ATR_THRESHOLD: f64 = 0.3;
pub const REACTION_FALLBACK_POINTS: f64 = 2.0;
/// Default measurement horizon used by the worker/report.
pub const DEFAULT_REACTION_HORIZON_MINUTES: i64 = 240;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventClass {
    Inflation,
    Labor,
    Growth,
    Consumption,
    Housing,
    Manufacturing,
    Sentiment,
    Fed,
    Geopolitical,
    #[default]
    Other,
}

impl EventClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Inflation => "INFLATION",
            Self::Labor => "LABOR",
            Self::Growth => "GROWTH",
            Self::Consumption => "CONSUMPTION",
            Self::Housing => "HOUSING",
            Self::Manufacturing => "MANUFACTURING",
            Self::Sentiment => "SENTIMENT",
            Self::Fed => "FED",
            Self::Geopolitical => "GEOPOLITICAL",
            Self::Other => "OTHER",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GoldRelevance {
    High,
    Medium,
    #[default]
    Low,
}

impl GoldRelevance {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "HIGH",
            Self::Medium => "MEDIUM",
            Self::Low => "LOW",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Lean {
    Up,
    Down,
    #[default]
    Unclear,
}

impl Lean {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::Unclear => "UNCLEAR",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Reaction {
    Up,
    Down,
    Flat,
    #[default]
    Unknown,
}

impl Reaction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::Flat => "FLAT",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HawkishDovish {
    Hawkish,
    Dovish,
    Neutral,
    #[default]
    Unknown,
}

impl HawkishDovish {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hawkish => "HAWKISH",
            Self::Dovish => "DOVISH",
            Self::Neutral => "NEUTRAL",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewsAiView {
    pub generated_at: DateTime<Utc>,
    pub model: String,
    pub prompt_version: String,
    pub event_class: EventClass,
    pub gold_relevance: GoldRelevance,
    pub hawkish_dovish: HawkishDovish,
    pub gold_lean: Lean,
    pub confidence: u8,
    #[serde(default)]
    pub conditions: Option<String>,
    pub reasoning: String,
}

impl Default for NewsAiView {
    fn default() -> Self {
        Self {
            generated_at: Utc::now(),
            model: String::new(),
            prompt_version: String::new(),
            event_class: EventClass::Other,
            gold_relevance: GoldRelevance::Low,
            hawkish_dovish: HawkishDovish::Unknown,
            gold_lean: Lean::Unclear,
            confidence: 0,
            conditions: None,
            reasoning: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewsImpactRecord {
    pub schema: u8,
    pub event_id: String,
    pub title: String,
    pub currency: String,
    pub impact: ImpactLevel,
    pub scheduled_time: DateTime<Utc>,
    pub event_class: EventClass,
    pub gold_relevance: GoldRelevance,
    pub forecast: Option<String>,
    pub previous: Option<String>,
    pub actual: Option<String>,
    pub surprise: Option<f64>,
    pub surprise_pct: Option<f64>,
    pub conditional_lean: Lean,
    pub conditional_reason: String,
    /// UPCOMING | MEASURING | COMPLETE | UNKNOWN
    pub status: String,
    pub baseline_price: Option<f64>,
    pub baseline_time: Option<DateTime<Utc>>,
    pub return_15m: Option<f64>,
    pub return_60m: Option<f64>,
    pub return_240m: Option<f64>,
    pub return_15m_pct: Option<f64>,
    pub return_60m_pct: Option<f64>,
    pub return_240m_pct: Option<f64>,
    pub mfe_points: Option<f64>,
    pub mae_points: Option<f64>,
    pub volume_ratio: Option<f64>,
    pub h4_atr: Option<f64>,
    pub reaction: Reaction,
    pub reaction_reason: String,
    pub price_source: String,
    pub volume_source: String,
    pub ai: Option<NewsAiView>,
    pub ai_attempted_at: Option<DateTime<Utc>>,
    pub ai_attempts: u8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Default for NewsImpactRecord {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            schema: NEWS_IMPACT_SCHEMA,
            event_id: String::new(),
            title: String::new(),
            currency: "USD".to_string(),
            impact: ImpactLevel::Medium,
            scheduled_time: now,
            event_class: EventClass::Other,
            gold_relevance: GoldRelevance::Low,
            forecast: None,
            previous: None,
            actual: None,
            surprise: None,
            surprise_pct: None,
            conditional_lean: Lean::Unclear,
            conditional_reason: String::new(),
            status: "UNKNOWN".to_string(),
            baseline_price: None,
            baseline_time: None,
            return_15m: None,
            return_60m: None,
            return_240m: None,
            return_15m_pct: None,
            return_60m_pct: None,
            return_240m_pct: None,
            mfe_points: None,
            mae_points: None,
            volume_ratio: None,
            h4_atr: None,
            reaction: Reaction::Unknown,
            reaction_reason: String::new(),
            price_source: String::new(),
            volume_source: String::new(),
            ai: None,
            ai_attempted_at: None,
            ai_attempts: 0,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NewsImpactBucket {
    pub event_class: EventClass,
    pub count: usize,
    pub up: usize,
    pub down: usize,
    pub flat: usize,
    pub unknown: usize,
    pub avg_abs_move_60m: Option<f64>,
    pub conditional_hit_rate: Option<f64>,
    pub ai_hit_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NewsImpactSummary {
    pub total_events: usize,
    pub measured_events: usize,
    pub up: usize,
    pub down: usize,
    pub flat: usize,
    pub unknown: usize,
    pub avg_abs_move_15m: Option<f64>,
    pub avg_abs_move_60m: Option<f64>,
    pub avg_abs_move_240m: Option<f64>,
    pub conditional_total: usize,
    pub conditional_correct: usize,
    pub conditional_hit_rate: Option<f64>,
    pub ai_total: usize,
    pub ai_correct: usize,
    pub ai_hit_rate: Option<f64>,
    pub majority_down_share: Option<f64>,
    pub by_class: Vec<NewsImpactBucket>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ConditionalLean {
    pub lean: Lean,
    pub surprise: Option<f64>,
    pub surprise_pct: Option<f64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ReactionMetrics {
    pub baseline_price: Option<f64>,
    pub baseline_time: Option<DateTime<Utc>>,
    pub return_15m: Option<f64>,
    pub return_60m: Option<f64>,
    pub return_240m: Option<f64>,
    pub return_15m_pct: Option<f64>,
    pub return_60m_pct: Option<f64>,
    pub return_240m_pct: Option<f64>,
    pub mfe_points: Option<f64>,
    pub mae_points: Option<f64>,
    pub volume_ratio: Option<f64>,
    pub reaction: Reaction,
    pub reason: String,
}

/// Context needed to build one record. Kept in a struct to avoid a long
/// argument list and to make tests explicit.
pub struct RecordContext<'a> {
    pub candles: &'a [Candle],
    pub h4_atr: Option<f64>,
    pub price_source: &'a str,
    pub volume_source: &'a str,
    pub horizon_minutes: i64,
    pub now: DateTime<Utc>,
}

/// Classify an event title into a coarse class and a gold relevance.
///
/// Order matters: "Philly Fed Manufacturing" must be manufacturing, and
/// "Inflation Expectations" must be sentiment before the generic inflation
/// matcher sees it.
pub fn classify_event(title: &str) -> (EventClass, GoldRelevance) {
    let lower = title.to_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|key| lower.contains(key));

    if has(&[
        "philly fed",
        "empire state",
        "ism manufacturing",
        "manufacturing pmi",
        "industrial production",
        "factory orders",
    ]) {
        return (EventClass::Manufacturing, GoldRelevance::Medium);
    }
    if has(&[
        "consumer sentiment",
        "consumer confidence",
        "inflation expectations",
        "michigan",
    ]) {
        return (EventClass::Sentiment, GoldRelevance::Medium);
    }
    if has(&[
        "fomc",
        "fed chair",
        "fed chairman",
        "powell",
        "warsh",
        "federal reserve",
        "interest rate decision",
        "monetary policy",
        "beige book",
        "meeting minutes",
    ]) {
        return (EventClass::Fed, GoldRelevance::High);
    }
    if has(&[
        "core cpi",
        "cpi",
        "core ppi",
        "ppi",
        "pce",
        "inflation",
        "consumer price",
        "producer price",
    ]) {
        return (EventClass::Inflation, GoldRelevance::High);
    }
    if has(&[
        "non-farm",
        "nonfarm",
        "nfp",
        "unemployment rate",
        "jobless claims",
        "unemployment claims",
        "adp",
        "average hourly earnings",
        "employment change",
        "payrolls",
    ]) {
        let medium = lower.contains("claims") || lower.contains("unemployment rate");
        return (
            EventClass::Labor,
            if medium {
                GoldRelevance::Medium
            } else {
                GoldRelevance::High
            },
        );
    }
    if has(&["gdp", "durable goods", "trade balance", "current account"]) {
        let high = lower.contains("gdp");
        return (
            EventClass::Growth,
            if high {
                GoldRelevance::High
            } else {
                GoldRelevance::Medium
            },
        );
    }
    if has(&[
        "retail sales",
        "personal spending",
        "consumer spending",
        "personal income",
    ]) {
        return (EventClass::Consumption, GoldRelevance::High);
    }
    if has(&[
        "housing starts",
        "building permits",
        "existing home sales",
        "new home sales",
        "case-shiller",
        "home price",
    ]) {
        return (EventClass::Housing, GoldRelevance::Medium);
    }
    if has(&[
        "war",
        "conflict",
        "sanction",
        "tariff",
        "election",
        "geopolitical",
        "opec",
        "middle east",
        "russia",
        "ukraine",
    ]) {
        return (EventClass::Geopolitical, GoldRelevance::High);
    }

    (EventClass::Other, GoldRelevance::Low)
}

/// Parse a ForexFactory-style numeric string (`0.4%`, `250K`, `-1.2`, `1.5B`).
/// Returns `None` for ranges, inequality prefixes, or non-numeric text.
pub fn parse_numeric(value: &str) -> Option<f64> {
    let mut text = value
        .trim()
        .replace(['−', '–'], "-")
        .replace([',', '%'], "");
    text = text.trim().to_string();
    if text.is_empty() || text.starts_with('<') || text.starts_with('>') || text.starts_with('~') {
        return None;
    }

    let upper = text.to_ascii_uppercase();
    let (number_part, multiplier) = if let Some(rest) = upper.strip_suffix('K') {
        (rest, 1_000.0)
    } else if let Some(rest) = upper.strip_suffix('M') {
        (rest, 1_000_000.0)
    } else if let Some(rest) = upper.strip_suffix('B') {
        (rest, 1_000_000_000.0)
    } else if let Some(rest) = upper.strip_suffix('T') {
        (rest, 1_000_000_000_000.0)
    } else {
        (upper.as_str(), 1.0)
    };

    let number_part = number_part.trim().trim_start_matches('+');
    let number = number_part.parse::<f64>().ok()?;
    if !number.is_finite() {
        return None;
    }
    Some(number * multiplier)
}

fn format_number(value: f64) -> String {
    if value.abs() >= 1_000.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Deterministic conditional lean from `actual - forecast`.
///
/// This is intentionally conservative: only event classes with a stable
/// macro→gold mapping get a direction. Fed speeches, sentiment and
/// geopolitical headlines stay `UNCLEAR`.
pub fn conditional_lean(
    event_class: EventClass,
    title: &str,
    actual: Option<&str>,
    forecast: Option<&str>,
) -> ConditionalLean {
    let Some(actual_value) = actual.and_then(parse_numeric) else {
        return ConditionalLean {
            lean: Lean::Unclear,
            surprise: None,
            surprise_pct: None,
            reason: "Thiếu actual dạng số — chưa thể xác định surprise.".to_string(),
        };
    };
    let Some(forecast_value) = forecast.and_then(parse_numeric) else {
        return ConditionalLean {
            lean: Lean::Unclear,
            surprise: None,
            surprise_pct: None,
            reason: "Thiếu forecast dạng số — chưa thể xác định surprise.".to_string(),
        };
    };

    let surprise = actual_value - forecast_value;
    let surprise_pct = if forecast_value.abs() > f64::EPSILON {
        Some(surprise / forecast_value.abs() * 100.0)
    } else {
        None
    };

    if surprise.abs() <= 1e-9 {
        return ConditionalLean {
            lean: Lean::Unclear,
            surprise: Some(0.0),
            surprise_pct: Some(0.0),
            reason: "Actual bằng forecast — không có surprise định lượng.".to_string(),
        };
    }

    let lower = title.to_lowercase();
    let higher_is_gold_positive = match event_class {
        EventClass::Labor => lower.contains("unemployment") || lower.contains("claims"),
        EventClass::Fed => {
            if lower.contains("rate decision") || lower.contains("interest rate") {
                false
            } else {
                return ConditionalLean {
                    lean: Lean::Unclear,
                    surprise: Some(surprise),
                    surprise_pct,
                    reason: "Fed speech/minutes không có mapping định lượng ổn định; cần ngữ cảnh."
                        .to_string(),
                };
            }
        }
        EventClass::Sentiment | EventClass::Geopolitical | EventClass::Other => {
            return ConditionalLean {
                lean: Lean::Unclear,
                surprise: Some(surprise),
                surprise_pct,
                reason: "Nhóm tin này không có mapping định lượng ổn định; cần AI/ngữ cảnh."
                    .to_string(),
            };
        }
        EventClass::Inflation
        | EventClass::Growth
        | EventClass::Consumption
        | EventClass::Housing
        | EventClass::Manufacturing => false,
    };

    let lean = if surprise > 0.0 {
        if higher_is_gold_positive {
            Lean::Up
        } else {
            Lean::Down
        }
    } else if higher_is_gold_positive {
        Lean::Down
    } else {
        Lean::Up
    };

    let channel = if higher_is_gold_positive {
        "lao động yếu → USD/lợi suất yếu → vàng được hỗ trợ"
    } else {
        "tăng trưởng/lạm phát mạnh → USD/lợi suất mạnh → vàng chịu áp lực"
    };
    ConditionalLean {
        lean,
        surprise: Some(surprise),
        surprise_pct,
        reason: format!(
            "Actual {} vs forecast {} (surprise {:+.2}) → {} → vàng nghiêng {}.",
            format_number(actual_value),
            format_number(forecast_value),
            surprise,
            channel,
            lean.as_str()
        ),
    }
}

fn pct_change(price: f64, baseline: f64) -> Option<f64> {
    if baseline.abs() <= f64::EPSILON || !price.is_finite() {
        None
    } else {
        Some((price - baseline) / baseline * 100.0)
    }
}

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn average_abs(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().map(|value| value.abs()).sum::<f64>() / values.len() as f64)
    }
}

fn matches_direction(lean: Lean, reaction: Reaction) -> bool {
    matches!(
        (lean, reaction),
        (Lean::Up, Reaction::Up) | (Lean::Down, Reaction::Down)
    )
}

/// Measure gold's reaction from closed M15 candles.
///
/// Baseline is the last M15 candle that closed at or before the event time.
/// A horizon is only filled when the candle series actually covers it, so a
/// partial +60m window never masquerades as +240m.
pub fn measure_reaction(
    event_time: DateTime<Utc>,
    candles: &[Candle],
    h4_atr: Option<f64>,
) -> ReactionMetrics {
    const STEP_MS: i64 = 15 * 60 * 1000;
    let event_ms = event_time.timestamp_millis();

    let mut bars: Vec<&Candle> = candles
        .iter()
        .filter(|candle| candle.timeframe == Timeframe::M15)
        .collect();
    bars.sort_by_key(|candle| candle.timestamp);
    bars.dedup_by_key(|candle| candle.timestamp);
    if bars.is_empty() {
        return ReactionMetrics {
            reaction: Reaction::Unknown,
            reason: "Chưa có nến M15 để đo phản ứng.".to_string(),
            ..ReactionMetrics::default()
        };
    }

    let Some(baseline_idx) = bars
        .iter()
        .rposition(|candle| candle.timestamp + STEP_MS <= event_ms)
    else {
        return ReactionMetrics {
            reaction: Reaction::Unknown,
            reason: "Không có nến M15 đóng cửa trước thời điểm tin.".to_string(),
            ..ReactionMetrics::default()
        };
    };
    let baseline = bars[baseline_idx];
    let baseline_price = baseline.close;
    let Some(last_close_ms) = bars.last().map(|candle| candle.timestamp + STEP_MS) else {
        return ReactionMetrics {
            reaction: Reaction::Unknown,
            reason: "Chưa có nến M15 đóng cửa.".to_string(),
            ..ReactionMetrics::default()
        };
    };

    let close_at = |target_ms: i64| -> Option<f64> {
        if last_close_ms < target_ms {
            return None;
        }
        bars.iter()
            .rev()
            .find(|candle| candle.timestamp + STEP_MS <= target_ms)
            .map(|candle| candle.close)
    };

    let return_15m = close_at(event_ms + 15 * 60 * 1000).map(|price| price - baseline_price);
    let return_60m = close_at(event_ms + 60 * 60 * 1000).map(|price| price - baseline_price);
    let return_240m = close_at(event_ms + 240 * 60 * 1000).map(|price| price - baseline_price);

    let window_end_ms = event_ms + 240 * 60 * 1000;
    let last_window_ms = window_end_ms.min(last_close_ms);
    let last_window_idx = bars
        .iter()
        .rposition(|candle| candle.timestamp + STEP_MS <= last_window_ms);
    let (mfe_points, mae_points) = match last_window_idx {
        Some(end_idx) if end_idx > baseline_idx => {
            let window = &bars[baseline_idx + 1..=end_idx];
            let high = window
                .iter()
                .map(|candle| candle.high)
                .fold(f64::NEG_INFINITY, f64::max);
            let low = window
                .iter()
                .map(|candle| candle.low)
                .fold(f64::INFINITY, f64::min);
            (
                Some((high - baseline_price).max(0.0)),
                Some((low - baseline_price).min(0.0)),
            )
        }
        _ => (None, None),
    };

    let after: Vec<f64> = bars
        .iter()
        .filter(|candle| candle.timestamp >= event_ms)
        .take(4)
        .map(|candle| candle.volume)
        .filter(|volume| volume.is_finite() && *volume > 0.0)
        .collect();
    let before: Vec<f64> = bars[..=baseline_idx]
        .iter()
        .rev()
        .take(20)
        .map(|candle| candle.volume)
        .filter(|volume| volume.is_finite() && *volume > 0.0)
        .collect();
    let volume_ratio = match (average(&after), average(&before)) {
        (Some(after_avg), Some(before_avg)) if before_avg > 0.0 => Some(after_avg / before_avg),
        _ => None,
    };

    let threshold = h4_atr
        .filter(|atr| atr.is_finite() && *atr > 0.0)
        .map(|atr| atr * REACTION_ATR_THRESHOLD)
        .unwrap_or(REACTION_FALLBACK_POINTS);
    let (reaction, reason) = match return_60m.or(return_15m) {
        Some(value) if value >= threshold => (
            Reaction::Up,
            format!(
                "+{value:.2} điểm (ngưỡng {threshold:.2}) tại {} → UP.",
                if return_60m.is_some() { "+60m" } else { "+15m" }
            ),
        ),
        Some(value) if value <= -threshold => (
            Reaction::Down,
            format!(
                "{value:.2} điểm (ngưỡng -{threshold:.2}) tại {} → DOWN.",
                if return_60m.is_some() { "+60m" } else { "+15m" }
            ),
        ),
        Some(value) => (
            Reaction::Flat,
            format!(
                "{value:+.2} điểm nằm trong ngưỡng ±{threshold:.2} tại {} → FLAT.",
                if return_60m.is_some() { "+60m" } else { "+15m" }
            ),
        ),
        None => (
            Reaction::Unknown,
            "Chưa đủ nến M15 sau tin để đo +15m/+60m.".to_string(),
        ),
    };

    ReactionMetrics {
        baseline_price: Some(baseline_price),
        baseline_time: Utc.timestamp_millis_opt(baseline.timestamp).single(),
        return_15m,
        return_60m,
        return_240m,
        return_15m_pct: return_15m
            .and_then(|value| pct_change(baseline_price + value, baseline_price)),
        return_60m_pct: return_60m
            .and_then(|value| pct_change(baseline_price + value, baseline_price)),
        return_240m_pct: return_240m
            .and_then(|value| pct_change(baseline_price + value, baseline_price)),
        mfe_points,
        mae_points,
        volume_ratio,
        reaction,
        reason,
    }
}

/// Merge fresh reaction metrics with the previous record so a short candle
/// window or a feed hiccup never erases a completed +60m/+240m measurement.
fn merge_reaction(existing: Option<&NewsImpactRecord>, fresh: ReactionMetrics) -> ReactionMetrics {
    let Some(old) = existing else {
        return fresh;
    };
    let keep_old_label = fresh.reaction == Reaction::Unknown && old.reaction != Reaction::Unknown;
    ReactionMetrics {
        baseline_price: fresh.baseline_price.or(old.baseline_price),
        baseline_time: fresh.baseline_time.or(old.baseline_time),
        return_15m: fresh.return_15m.or(old.return_15m),
        return_60m: fresh.return_60m.or(old.return_60m),
        return_240m: fresh.return_240m.or(old.return_240m),
        return_15m_pct: fresh.return_15m_pct.or(old.return_15m_pct),
        return_60m_pct: fresh.return_60m_pct.or(old.return_60m_pct),
        return_240m_pct: fresh.return_240m_pct.or(old.return_240m_pct),
        mfe_points: fresh.mfe_points.or(old.mfe_points),
        mae_points: fresh.mae_points.or(old.mae_points),
        volume_ratio: fresh.volume_ratio.or(old.volume_ratio),
        reaction: if keep_old_label {
            old.reaction
        } else {
            fresh.reaction
        },
        reason: if keep_old_label {
            old.reaction_reason.clone()
        } else {
            fresh.reason
        },
    }
}

fn non_empty_or(value: String, fallback: Option<String>) -> String {
    if value.trim().is_empty() {
        fallback.unwrap_or(value)
    } else {
        value
    }
}

/// Build (or refresh) one record from a calendar event.
pub fn build_record(
    event: &EconomicEvent,
    existing: Option<&NewsImpactRecord>,
    context: &RecordContext<'_>,
) -> NewsImpactRecord {
    let (event_class, gold_relevance) = classify_event(&event.title);
    let forecast = event
        .forecast
        .clone()
        .or_else(|| existing.and_then(|record| record.forecast.clone()));
    let previous = event
        .previous
        .clone()
        .or_else(|| existing.and_then(|record| record.previous.clone()));
    let actual = event
        .actual
        .clone()
        .or_else(|| existing.and_then(|record| record.actual.clone()));
    let conditional = conditional_lean(
        event_class,
        &event.title,
        actual.as_deref(),
        forecast.as_deref(),
    );
    let reaction = merge_reaction(
        existing,
        measure_reaction(event.scheduled_time, context.candles, context.h4_atr),
    );
    let created_at = existing
        .map(|record| record.created_at)
        .unwrap_or(context.now);
    let status = if context.now < event.scheduled_time {
        "UPCOMING"
    } else if context.now
        <= event.scheduled_time + Duration::minutes(context.horizon_minutes.clamp(60, 240))
    {
        "MEASURING"
    } else {
        "COMPLETE"
    };
    let h4_atr = context
        .h4_atr
        .or_else(|| existing.and_then(|record| record.h4_atr));

    NewsImpactRecord {
        schema: NEWS_IMPACT_SCHEMA,
        event_id: event.id.clone(),
        title: event.title.clone(),
        currency: event.currency.clone(),
        impact: event.impact,
        scheduled_time: event.scheduled_time,
        event_class,
        gold_relevance,
        forecast,
        previous,
        actual,
        surprise: conditional.surprise,
        surprise_pct: conditional.surprise_pct,
        conditional_lean: conditional.lean,
        conditional_reason: conditional.reason,
        status: status.to_string(),
        baseline_price: reaction.baseline_price,
        baseline_time: reaction.baseline_time,
        return_15m: reaction.return_15m,
        return_60m: reaction.return_60m,
        return_240m: reaction.return_240m,
        return_15m_pct: reaction.return_15m_pct,
        return_60m_pct: reaction.return_60m_pct,
        return_240m_pct: reaction.return_240m_pct,
        mfe_points: reaction.mfe_points,
        mae_points: reaction.mae_points,
        volume_ratio: reaction.volume_ratio,
        h4_atr,
        reaction: reaction.reaction,
        reaction_reason: reaction.reason,
        price_source: non_empty_or(
            context.price_source.to_string(),
            existing.map(|record| record.price_source.clone()),
        ),
        volume_source: non_empty_or(
            context.volume_source.to_string(),
            existing.map(|record| record.volume_source.clone()),
        ),
        ai: existing.and_then(|record| record.ai.clone()),
        ai_attempted_at: existing.and_then(|record| record.ai_attempted_at),
        ai_attempts: existing.map(|record| record.ai_attempts).unwrap_or(0),
        created_at,
        updated_at: context.now,
    }
}

/// Insert or replace a record by `event_id`, keep chronological order and cap
/// the store so a long-running bot cannot grow the file without bound.
pub fn upsert_record(records: &mut Vec<NewsImpactRecord>, record: NewsImpactRecord) {
    if let Some(existing) = records
        .iter_mut()
        .find(|existing| existing.event_id == record.event_id)
    {
        *existing = record;
    } else {
        records.push(record);
    }
    records.sort_by_key(|record| record.scheduled_time);
    if records.len() > MAX_NEWS_IMPACT_RECORDS {
        let excess = records.len() - MAX_NEWS_IMPACT_RECORDS;
        records.drain(0..excess);
    }
}

fn hit_rate(correct: usize, total: usize) -> Option<f64> {
    if total == 0 {
        None
    } else {
        Some(correct as f64 / total as f64)
    }
}

fn bucket(records: &[&NewsImpactRecord]) -> NewsImpactBucket {
    let mut result = NewsImpactBucket {
        event_class: records
            .first()
            .map(|record| record.event_class)
            .unwrap_or(EventClass::Other),
        count: records.len(),
        ..NewsImpactBucket::default()
    };
    let mut moves_60m = Vec::new();
    let mut conditional_total = 0;
    let mut conditional_correct = 0;
    let mut ai_total = 0;
    let mut ai_correct = 0;

    for record in records {
        match record.reaction {
            Reaction::Up => result.up += 1,
            Reaction::Down => result.down += 1,
            Reaction::Flat => result.flat += 1,
            Reaction::Unknown => result.unknown += 1,
        }
        if let Some(value) = record.return_60m {
            if value.is_finite() {
                moves_60m.push(value);
            }
        }
        if record.conditional_lean != Lean::Unclear
            && matches!(record.reaction, Reaction::Up | Reaction::Down)
        {
            conditional_total += 1;
            if matches_direction(record.conditional_lean, record.reaction) {
                conditional_correct += 1;
            }
        }
        if let Some(ai) = record.ai.as_ref() {
            if ai.gold_lean != Lean::Unclear
                && matches!(record.reaction, Reaction::Up | Reaction::Down)
            {
                ai_total += 1;
                if matches_direction(ai.gold_lean, record.reaction) {
                    ai_correct += 1;
                }
            }
        }
    }

    result.avg_abs_move_60m = average_abs(&moves_60m);
    result.conditional_hit_rate = hit_rate(conditional_correct, conditional_total);
    result.ai_hit_rate = hit_rate(ai_correct, ai_total);
    result
}

/// Transparent summary used by the API, dashboard and CLI report.
pub fn summarize(records: &[NewsImpactRecord]) -> NewsImpactSummary {
    let mut summary = NewsImpactSummary {
        total_events: records.len(),
        ..NewsImpactSummary::default()
    };
    let mut moves_15m = Vec::new();
    let mut moves_60m = Vec::new();
    let mut moves_240m = Vec::new();
    let mut conditional_total = 0;
    let mut conditional_correct = 0;
    let mut ai_total = 0;
    let mut ai_correct = 0;
    let mut grouped: BTreeMap<EventClass, Vec<&NewsImpactRecord>> = BTreeMap::new();

    for record in records {
        match record.reaction {
            Reaction::Up => summary.up += 1,
            Reaction::Down => summary.down += 1,
            Reaction::Flat => summary.flat += 1,
            Reaction::Unknown => summary.unknown += 1,
        }
        if record.reaction != Reaction::Unknown {
            summary.measured_events += 1;
        }
        if let Some(value) = record.return_15m.filter(|value| value.is_finite()) {
            moves_15m.push(value);
        }
        if let Some(value) = record.return_60m.filter(|value| value.is_finite()) {
            moves_60m.push(value);
        }
        if let Some(value) = record.return_240m.filter(|value| value.is_finite()) {
            moves_240m.push(value);
        }
        if record.conditional_lean != Lean::Unclear
            && matches!(record.reaction, Reaction::Up | Reaction::Down)
        {
            conditional_total += 1;
            if matches_direction(record.conditional_lean, record.reaction) {
                conditional_correct += 1;
            }
        }
        if let Some(ai) = record.ai.as_ref() {
            if ai.gold_lean != Lean::Unclear
                && matches!(record.reaction, Reaction::Up | Reaction::Down)
            {
                ai_total += 1;
                if matches_direction(ai.gold_lean, record.reaction) {
                    ai_correct += 1;
                }
            }
        }
        grouped.entry(record.event_class).or_default().push(record);
    }

    summary.avg_abs_move_15m = average_abs(&moves_15m);
    summary.avg_abs_move_60m = average_abs(&moves_60m);
    summary.avg_abs_move_240m = average_abs(&moves_240m);
    summary.conditional_total = conditional_total;
    summary.conditional_correct = conditional_correct;
    summary.conditional_hit_rate = hit_rate(conditional_correct, conditional_total);
    summary.ai_total = ai_total;
    summary.ai_correct = ai_correct;
    summary.ai_hit_rate = hit_rate(ai_correct, ai_total);
    let directional = summary.up + summary.down;
    summary.majority_down_share = if directional == 0 {
        None
    } else {
        Some(summary.down as f64 / directional as f64)
    };
    summary.by_class = grouped
        .into_values()
        .map(|records| bucket(&records))
        .collect();
    summary
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct NewsImpactFile {
    #[serde(default)]
    schema: u8,
    #[serde(default)]
    records: Vec<NewsImpactRecord>,
}

/// Atomic JSON store for `data/news_reactions.json`.
#[derive(Debug, Clone)]
pub struct NewsImpactStore {
    path: PathBuf,
}

impl NewsImpactStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Missing/corrupt file => empty store, never panic.
    pub fn load(&self) -> Vec<NewsImpactRecord> {
        if !self.path.exists() {
            info!(
                "ℹ️ Không tìm thấy file news impact {}, khởi tạo rỗng.",
                self.path.display()
            );
            return Vec::new();
        }
        match fs::read_to_string(&self.path) {
            Ok(content) => match serde_json::from_str::<NewsImpactFile>(&content) {
                Ok(file) => {
                    info!(
                        "✅ Đã nạp {} news impact record từ {}",
                        file.records.len(),
                        self.path.display()
                    );
                    file.records
                }
                Err(err) => {
                    warn!(
                        "⚠️ File news impact {} không hợp lệ ({}), dùng rỗng.",
                        self.path.display(),
                        err
                    );
                    Vec::new()
                }
            },
            Err(err) => {
                warn!(
                    "⚠️ Không đọc được file news impact {} ({}), dùng rỗng.",
                    self.path.display(),
                    err
                );
                Vec::new()
            }
        }
    }

    pub fn save(&self, records: &[NewsImpactRecord]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|err| {
                    format!("Không tạo được thư mục {}: {}", parent.display(), err)
                })?;
            }
        }
        let file = NewsImpactFile {
            schema: NEWS_IMPACT_SCHEMA,
            records: records.to_vec(),
        };
        let data = serde_json::to_string_pretty(&file)
            .map_err(|err| format!("Không serialize được news impact: {}", err))?;
        let tmp_path = self.path.with_extension("json.tmp");
        fs::write(&tmp_path, data)
            .map_err(|err| format!("Không ghi được file tạm {}: {}", tmp_path.display(), err))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
        }

        fs::rename(&tmp_path, &self.path).map_err(|err| {
            format!(
                "Không thể thay thế file news impact {}: {}",
                self.path.display(),
                err
            )
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid datetime")
    }

    fn candle(timestamp: i64, close: f64, high: f64, low: f64, volume: f64) -> Candle {
        Candle {
            timestamp,
            open: close,
            high,
            low,
            close,
            volume,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    #[test]
    fn classifies_philly_fed_as_manufacturing_not_fed() {
        let (class, relevance) = classify_event("Philly Fed Manufacturing Index");
        assert_eq!(class, EventClass::Manufacturing);
        assert_eq!(relevance, GoldRelevance::Medium);
    }

    #[test]
    fn classifies_inflation_expectations_as_sentiment() {
        let (class, _) = classify_event("Revised UoM Inflation Expectations");
        assert_eq!(class, EventClass::Sentiment);
    }

    #[test]
    fn classifies_fed_chair_and_fomc_as_fed() {
        assert_eq!(
            classify_event("Fed Chairman Warsh Speaks").0,
            EventClass::Fed
        );
        assert_eq!(classify_event("FOMC Meeting Minutes").0, EventClass::Fed);
    }

    #[test]
    fn classifies_retail_sales_as_consumption_high() {
        let (class, relevance) = classify_event("Core Retail Sales m/m");
        assert_eq!(class, EventClass::Consumption);
        assert_eq!(relevance, GoldRelevance::High);
    }

    #[test]
    fn parses_percent_and_suffix_numbers() {
        assert_eq!(parse_numeric("0.4%"), Some(0.4));
        assert_eq!(parse_numeric("250K"), Some(250_000.0));
        assert_eq!(parse_numeric("-1.2B"), Some(-1_200_000_000.0));
        assert_eq!(parse_numeric("+3"), Some(3.0));
        assert_eq!(parse_numeric("<0.1%"), None);
        assert_eq!(parse_numeric("n/a"), None);
    }

    #[test]
    fn inflation_beat_is_gold_down() {
        let lean = conditional_lean(
            EventClass::Inflation,
            "Core CPI m/m",
            Some("0.5%"),
            Some("0.3%"),
        );
        assert_eq!(lean.lean, Lean::Down);
        assert!(lean.surprise.unwrap() > 0.0);
    }

    #[test]
    fn jobless_claims_beat_is_gold_up() {
        let lean = conditional_lean(
            EventClass::Labor,
            "Unemployment Claims",
            Some("260K"),
            Some("220K"),
        );
        assert_eq!(lean.lean, Lean::Up);
    }

    #[test]
    fn inline_actual_is_unclear() {
        let lean = conditional_lean(
            EventClass::Consumption,
            "Retail Sales m/m",
            Some("0.4%"),
            Some("0.4%"),
        );
        assert_eq!(lean.lean, Lean::Unclear);
        assert_eq!(lean.surprise, Some(0.0));
    }

    #[test]
    fn fed_speech_is_unclear_without_text() {
        let lean = conditional_lean(
            EventClass::Fed,
            "Fed Chairman Warsh Speaks",
            Some("5.25%"),
            Some("5.25%"),
        );
        assert_eq!(lean.lean, Lean::Unclear);
    }

    #[test]
    fn reaction_measures_returns_and_labels_up() {
        let event = at(2026, 11, 17, 13, 30);
        let event_ms = event.timestamp_millis();
        let step = 15 * 60 * 1000;
        let candles: Vec<Candle> = (0..20)
            .map(|index| {
                let ts = event_ms - 2 * step + index * step;
                let close = 100.0 + index as f64;
                candle(ts, close, close + 1.0, close - 1.0, 100.0)
            })
            .collect();
        let metrics = measure_reaction(event, &candles, Some(10.0));
        assert!(metrics.return_15m.unwrap() > 0.0);
        assert!(metrics.return_60m.unwrap() > 0.0);
        assert_eq!(metrics.reaction, Reaction::Up);
        assert!(metrics.volume_ratio.is_some());
    }

    #[test]
    fn reaction_is_unknown_without_baseline() {
        let event = at(2026, 11, 17, 13, 30);
        let event_ms = event.timestamp_millis();
        let candles = vec![candle(event_ms + 15 * 60 * 1000, 100.0, 101.0, 99.0, 10.0)];
        let metrics = measure_reaction(event, &candles, Some(10.0));
        assert_eq!(metrics.reaction, Reaction::Unknown);
        assert!(metrics.return_15m.is_none());
    }

    #[test]
    fn partial_window_does_not_fake_240m() {
        let event = at(2026, 11, 17, 13, 30);
        let event_ms = event.timestamp_millis();
        let step = 15 * 60 * 1000;
        let candles: Vec<Candle> = (0..5)
            .map(|index| {
                let ts = event_ms - step + index * step;
                candle(ts, 100.0 + index as f64, 101.0, 99.0, 10.0)
            })
            .collect();
        let metrics = measure_reaction(event, &candles, Some(10.0));
        assert!(metrics.return_15m.is_some());
        assert!(metrics.return_60m.is_some());
        assert!(metrics.return_240m.is_none());
    }

    #[test]
    fn build_record_preserves_actual_and_completed_reaction() {
        let event_time = at(2026, 11, 17, 13, 30);
        let existing = NewsImpactRecord {
            event_id: "FF-1".to_string(),
            title: "Core CPI m/m".to_string(),
            actual: Some("0.5%".to_string()),
            forecast: Some("0.3%".to_string()),
            return_60m: Some(-8.0),
            return_60m_pct: Some(-0.2),
            reaction: Reaction::Down,
            reaction_reason: "old measurement".to_string(),
            h4_atr: Some(18.0),
            price_source: "TWELVEDATA".to_string(),
            volume_source: "YAHOO_FUTURES_PROXY".to_string(),
            ..NewsImpactRecord::default()
        };
        let mut event = EconomicEvent {
            id: "FF-1".to_string(),
            title: "Core CPI m/m".to_string(),
            currency: "USD".to_string(),
            impact: ImpactLevel::High,
            scheduled_time: event_time,
            forecast: None,
            previous: None,
            actual: None,
        };
        let context = RecordContext {
            candles: &[],
            h4_atr: None,
            price_source: "",
            volume_source: "",
            horizon_minutes: 240,
            now: at(2026, 11, 17, 18, 0),
        };
        let record = build_record(&event, Some(&existing), &context);
        assert_eq!(record.actual.as_deref(), Some("0.5%"));
        assert_eq!(record.forecast.as_deref(), Some("0.3%"));
        assert_eq!(record.return_60m, Some(-8.0));
        assert_eq!(record.reaction, Reaction::Down);
        assert_eq!(record.h4_atr, Some(18.0));
        assert_eq!(record.price_source, "TWELVEDATA");

        event.actual = Some("0.6%".to_string());
        let refreshed = build_record(&event, Some(&record), &context);
        assert_eq!(refreshed.actual.as_deref(), Some("0.6%"));
        assert_eq!(refreshed.conditional_lean, Lean::Down);
    }

    #[test]
    fn summary_counts_and_hit_rates() {
        let mut first = NewsImpactRecord {
            event_id: "A".to_string(),
            event_class: EventClass::Inflation,
            conditional_lean: Lean::Down,
            reaction: Reaction::Down,
            return_60m: Some(-8.0),
            ..NewsImpactRecord::default()
        };
        first.ai = Some(NewsAiView {
            gold_lean: Lean::Down,
            ..NewsAiView::default()
        });
        let second = NewsImpactRecord {
            event_id: "B".to_string(),
            event_class: EventClass::Inflation,
            conditional_lean: Lean::Up,
            reaction: Reaction::Up,
            return_60m: Some(6.0),
            ..NewsImpactRecord::default()
        };
        let summary = summarize(&[first, second]);
        assert_eq!(summary.total_events, 2);
        assert_eq!(summary.measured_events, 2);
        assert_eq!(summary.conditional_total, 2);
        assert_eq!(summary.conditional_correct, 2);
        assert_eq!(summary.conditional_hit_rate, Some(1.0));
        assert_eq!(summary.ai_total, 1);
        assert_eq!(summary.ai_hit_rate, Some(1.0));
        assert_eq!(summary.avg_abs_move_60m, Some(7.0));
    }

    #[test]
    fn store_round_trip_and_upsert_cap() {
        let dir = std::env::temp_dir().join(format!(
            "news-impact-test-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let path = dir.join("news_reactions.json");
        let store = NewsImpactStore::new(&path);
        let mut records = Vec::new();
        let record = NewsImpactRecord {
            event_id: "A".to_string(),
            title: "Core CPI".to_string(),
            ..NewsImpactRecord::default()
        };
        upsert_record(&mut records, record);
        store.save(&records).expect("save");
        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].event_id, "A");

        let replacement = NewsImpactRecord {
            event_id: "A".to_string(),
            title: "Core CPI revised".to_string(),
            ..NewsImpactRecord::default()
        };
        upsert_record(&mut records, replacement);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].title, "Core CPI revised");
        let _ = fs::remove_dir_all(&dir);
    }
}
