use super::AppState;
use crate::storage::PersistentStore;
use crate::tracker::compute_stats;
use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;
use serde_json::json;

/// Lấy danh sách các tín hiệu đã sinh ra + trạng thái quota Sniper trong ngày.
pub(super) async fn get_all_signals(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let signals = state.signals.read().await.clone();
    let now = Utc::now();
    let signals_today = PersistentStore::count_live_signals_today(&signals, now);
    let daily_quota_reached =
        PersistentStore::has_reached_daily_quota(&signals, config.max_signals_per_day, now);
    let stats = compute_stats(&signals);

    Json(json!({
        "total": signals.len(),
        "signals": signals,
        "stats": stats,
        "signalsToday": signals_today,
        "maxSignalsPerDay": config.max_signals_per_day,
        "dailyQuotaReached": daily_quota_reached
    }))
}

/// Hiệu suất tích lũy của các tín hiệu thật (TP1/TP2/SL).
pub(super) async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    let signals = state.signals.read().await.clone();
    Json(compute_stats(&signals))
}
