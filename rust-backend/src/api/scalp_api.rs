//! Read-only scalp stats + editable scalp filter config.
//!
//! These endpoints are only used by the `/scalp` page. They never change swing
//! config, signals, quota or Telegram.

use crate::api::AppState;
use crate::scalp_config::{ScalpFilterConfig, ScalpFilterStore};
use crate::trigger_journal::TriggerEvent;
use crate::volume_profile;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};

const COST_USD: f64 = 0.5;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScalpStats {
    pub alerts: usize,
    pub open: usize,
    pub tp1_open: usize,
    pub completed: usize,
    pub wins: usize,
    pub win_rate: f64,
    pub avg_r: f64,
    pub total_r: f64,
    pub pf: Option<f64>,
    pub max_dd: f64,
    pub alerts_today: usize,
    pub alerts_7d: usize,
    pub last_alert_at: Option<DateTime<Utc>>,
    pub tp2: usize,
    pub tp1_be: usize,
    pub sl: usize,
    pub timeout: usize,
    pub skipped: HashMap<String, usize>,
}

fn vn_date(value: DateTime<Utc>) -> NaiveDate {
    (value + Duration::hours(7)).date_naive()
}

fn net_r(event: &TriggerEvent) -> Option<f64> {
    let gross = event.r_multiple?;
    let risk = event.risk_points.filter(|value| *value > 0.0)?;
    Some(gross - COST_USD / risk)
}

pub fn compute_scalp_stats(events: &[TriggerEvent], now: DateTime<Utc>) -> ScalpStats {
    let mut stats = ScalpStats::default();
    let mut alerts: HashMap<String, TriggerEvent> = HashMap::new();
    let mut outcomes: HashMap<String, TriggerEvent> = HashMap::new();
    let mut tp1: HashSet<String> = HashSet::new();

    for event in events {
        match event.event.as_str() {
            "scalp_ob_alert" => {
                if let Some(setup_id) = event.setup_id.clone() {
                    alerts.insert(setup_id, event.clone());
                }
            }
            "scalp_ob_outcome" => {
                if let Some(setup_id) = event.setup_id.clone() {
                    outcomes.insert(setup_id, event.clone());
                }
            }
            "scalp_ob_tp1" => {
                if let Some(setup_id) = event.setup_id.clone() {
                    tp1.insert(setup_id);
                }
            }
            "scalp_ob_skipped" => {
                let reason = event
                    .reason
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                *stats.skipped.entry(reason).or_insert(0) += 1;
            }
            _ => {}
        }
    }

    stats.alerts = alerts.len();
    let today = vn_date(now);
    for alert in alerts.values() {
        let Some(created_at) = alert.created_at else {
            continue;
        };
        if vn_date(created_at) == today {
            stats.alerts_today += 1;
        }
        if now - created_at <= Duration::days(7) {
            stats.alerts_7d += 1;
        }
        if stats
            .last_alert_at
            .map(|last| created_at > last)
            .unwrap_or(true)
        {
            stats.last_alert_at = Some(created_at);
        }
        if !outcomes.contains_key(alert.setup_id.as_deref().unwrap_or_default()) {
            stats.open += 1;
            if alert
                .setup_id
                .as_ref()
                .is_some_and(|setup_id| tp1.contains(setup_id))
            {
                stats.tp1_open += 1;
            }
        }
    }

    let mut completed: Vec<(DateTime<Utc>, f64)> = Vec::new();
    for (setup_id, outcome) in &outcomes {
        let Some(value) = net_r(outcome) else {
            continue;
        };
        let created_at = outcome.created_at.unwrap_or(now);
        completed.push((created_at, value));
        match outcome.reason.as_deref().unwrap_or_default() {
            "TP2" => stats.tp2 += 1,
            "TP1_BE" => stats.tp1_be += 1,
            "SL" => stats.sl += 1,
            "TIMEOUT" => stats.timeout += 1,
            _ => {}
        }
        if value > 0.0 {
            stats.wins += 1;
        }
        let _ = setup_id;
    }
    completed.sort_by_key(|(created_at, _)| *created_at);
    stats.completed = completed.len();
    if stats.completed > 0 {
        stats.win_rate = stats.wins as f64 / stats.completed as f64 * 100.0;
        stats.total_r = completed.iter().map(|(_, value)| *value).sum();
        stats.avg_r = stats.total_r / stats.completed as f64;
        let gross_win: f64 = completed
            .iter()
            .filter(|(_, value)| *value > 0.0)
            .map(|(_, value)| *value)
            .sum();
        let gross_loss: f64 = completed
            .iter()
            .filter(|(_, value)| *value < 0.0)
            .map(|(_, value)| -*value)
            .sum();
        stats.pf = (gross_loss > 0.0).then_some(gross_win / gross_loss);
        let mut equity: f64 = 0.0;
        let mut peak: f64 = 0.0;
        let mut max_dd: f64 = 0.0;
        for (_, value) in &completed {
            equity += value;
            peak = peak.max(equity);
            max_dd = max_dd.max(peak - equity);
        }
        stats.max_dd = max_dd;
    }

    stats
}

pub(super) async fn get_scalp_filters() -> impl IntoResponse {
    let store = ScalpFilterStore::from_env();
    Json(store.load())
}

pub(super) async fn update_scalp_filters(
    Json(config): Json<ScalpFilterConfig>,
) -> impl IntoResponse {
    let store = ScalpFilterStore::from_env();
    let sanitized = config.sanitized();
    match store.save(&sanitized) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "status": "success",
                "filters": sanitized
            })),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "message": err
            })),
        ),
    }
}

pub(super) async fn get_scalp_stats(State(state): State<AppState>) -> impl IntoResponse {
    let events = state.trigger_journal.load(None);
    Json(compute_scalp_stats(&events, Utc::now()))
}

#[derive(Debug, Clone, Serialize)]
pub struct ScalpVolumeProfile {
    pub poc: f64,
    pub val: f64,
    pub vah: f64,
    pub price: f64,
    pub position: String,
    pub bars: usize,
    pub lookback: usize,
    pub bins: usize,
    pub value_area: f64,
    pub timeframe: String,
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub(super) async fn get_scalp_volume_profile(State(state): State<AppState>) -> impl IntoResponse {
    let candles = state.candles.read().await.clone();
    let live_price = *state.live_price.read().await;
    let price = live_price.or_else(|| candles.last().map(|candle| candle.close));
    let Some(profile) = volume_profile::compute(
        &candles,
        volume_profile::DEFAULT_LOOKBACK,
        volume_profile::DEFAULT_BINS,
        volume_profile::DEFAULT_VALUE_AREA,
    ) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "insufficient_data"})),
        )
            .into_response();
    };
    let Some(price) = price.filter(|value| value.is_finite() && *value > 0.0) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status": "no_price"})),
        )
            .into_response();
    };
    let position = if price > profile.vah {
        "above_va"
    } else if price < profile.val {
        "below_va"
    } else {
        "inside_va"
    };
    (
        StatusCode::OK,
        Json(ScalpVolumeProfile {
            poc: round2(profile.poc),
            val: round2(profile.val),
            vah: round2(profile.vah),
            price: round2(price),
            position: position.to_string(),
            bars: profile.bars,
            lookback: volume_profile::DEFAULT_LOOKBACK,
            bins: volume_profile::DEFAULT_BINS,
            value_area: volume_profile::DEFAULT_VALUE_AREA,
            timeframe: "M15".to_string(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).single().unwrap()
    }

    fn alert(setup_id: &str, created_at: DateTime<Utc>) -> TriggerEvent {
        let mut event = TriggerEvent::new("scalp_ob_alert");
        event.setup_id = Some(setup_id.to_string());
        event.created_at = Some(created_at);
        event.risk_points = Some(10.0);
        event
    }

    fn outcome(setup_id: &str, reason: &str, r: f64, created_at: DateTime<Utc>) -> TriggerEvent {
        let mut event = TriggerEvent::new("scalp_ob_outcome");
        event.setup_id = Some(setup_id.to_string());
        event.created_at = Some(created_at);
        event.risk_points = Some(10.0);
        event.r_multiple = Some(r);
        event.reason = Some(reason.to_string());
        event
    }

    #[test]
    fn stats_count_alerts_and_outcomes() {
        let now = at(2026, 11, 19, 12);
        let events = vec![
            alert("S1", at(2026, 11, 19, 8)),
            alert("S2", at(2026, 11, 18, 8)),
            alert("S3", at(2026, 11, 19, 9)),
            outcome("S1", "TP2", 2.0, at(2026, 11, 19, 10)),
            outcome("S2", "SL", -1.0, at(2026, 11, 18, 10)),
        ];
        let stats = compute_scalp_stats(&events, now);
        assert_eq!(stats.alerts, 3);
        assert_eq!(stats.open, 1);
        assert_eq!(stats.completed, 2);
        assert_eq!(stats.wins, 1);
        assert_eq!(stats.tp2, 1);
        assert_eq!(stats.sl, 1);
        assert_eq!(stats.alerts_today, 2);
        assert!(stats.total_r > 0.0);
        assert_eq!(stats.tp1_open, 0);
    }

    #[test]
    fn stats_count_tp1_open_and_skips() {
        let now = at(2026, 11, 19, 12);
        let mut tp1 = TriggerEvent::new("scalp_ob_tp1");
        tp1.setup_id = Some("S1".to_string());
        tp1.created_at = Some(at(2026, 11, 19, 10));
        let mut skipped = TriggerEvent::new("scalp_ob_skipped");
        skipped.setup_id = Some("S2".to_string());
        skipped.reason = Some("ob_too_far".to_string());
        let events = vec![alert("S1", at(2026, 11, 19, 8)), tp1, skipped];
        let stats = compute_scalp_stats(&events, now);
        assert_eq!(stats.open, 1);
        assert_eq!(stats.tp1_open, 1);
        assert_eq!(stats.skipped.get("ob_too_far"), Some(&1));
    }
}
