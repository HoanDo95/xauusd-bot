use super::{persist_state, AppState};
use crate::models::{Signal, SignalStatus};
use crate::storage::PersistentStore;
use crate::tracker::compute_stats;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;

/// Lấy danh sách các tín hiệu đã sinh ra + trạng thái quota Sniper trong ngày.
pub(super) async fn get_all_signals(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let signals = state.signals.read().await.clone();
    let now = Utc::now();
    let signals_today = PersistentStore::count_live_signals_today(&signals, now);
    let daily_quota_reached = config.daily_quota_enabled
        && PersistentStore::has_reached_daily_quota(&signals, config.max_signals_per_day, now);
    let stats = compute_stats(&signals);

    Json(json!({
        "total": signals.len(),
        "signals": signals,
        "stats": stats,
        "signalsToday": signals_today,
        "maxSignalsPerDay": config.max_signals_per_day,
        "dailyQuotaEnabled": config.daily_quota_enabled,
        "dailyQuotaReached": daily_quota_reached
    }))
}

/// Hiệu suất tích lũy của các tín hiệu thật (TP1/TP2/SL).
pub(super) async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    let signals = state.signals.read().await.clone();
    Json(compute_stats(&signals))
}

#[derive(Debug, Deserialize)]
pub(super) struct CloseAllQuery {
    /// `?purge=true` removes the whole signal history instead of only closing
    /// active signals. Intended for clearing test data before forward testing.
    #[serde(default)]
    pub purge: bool,
}

/// POST /api/signals/close-all — close every Active signal as Cancelled.
/// With `?purge=true`, remove all signals from the store instead.
///
/// This is an operator/admin action. It never sends Telegram and never creates
/// a trade signal; it only updates the local signal journal.
pub(super) async fn close_all_signals(
    State(state): State<AppState>,
    Query(query): Query<CloseAllQuery>,
) -> impl IntoResponse {
    let now = Utc::now();
    let (closed, purged) = {
        let mut signals = state.signals.write().await;
        if query.purge {
            let removed = signals.len();
            signals.clear();
            (0, removed)
        } else {
            (close_active_signals(&mut signals, now), 0)
        }
    };

    if let Err(err) = persist_state(&state).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": err})),
        );
    }

    let remaining = state.signals.read().await.len();
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "closed": closed,
            "purged": purged,
            "remaining": remaining,
            "at": now.to_rfc3339(),
        })),
    )
}

fn close_active_signals(signals: &mut [Signal], now: DateTime<Utc>) -> usize {
    let mut closed = 0;
    for signal in signals.iter_mut() {
        if signal.status == SignalStatus::Active {
            signal.status = SignalStatus::Cancelled;
            signal.outcome_at = Some(now);
            signal.cancel_reason = Some("manual close-all: test data cleanup".to_string());
            closed += 1;
        }
    }
    closed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EntryType, SignalType, Timeframe};

    fn signal(status: SignalStatus, simulated: bool) -> Signal {
        let now = Utc::now();
        Signal {
            id: format!("SIG-{status:?}-{simulated}"),
            symbol: "XAU/USD".to_string(),
            signal_type: SignalType::Buy,
            timeframe: Timeframe::M15,
            entry_price: 2000.0,
            stop_loss: 1990.0,
            take_profit_1: 2020.0,
            take_profit_2: 2030.0,
            risk_points: 10.0,
            reward_points_tp1: 20.0,
            reward_points_tp2: 30.0,
            risk_reward_ratio: 2.0,
            volume_surge_ratio: 1.5,
            recommended_lot_size: 0.1,
            risk_amount_usd: 100.0,
            potential_profit_tp1_usd: 200.0,
            potential_profit_tp2_usd: 300.0,
            price_action_notes: String::new(),
            volume_analysis_notes: String::new(),
            full_rationale: String::new(),
            status,
            created_at: now,
            telegram_message_id: None,
            dedup_key: String::new(),
            candle_timestamp: 0,
            is_simulated: simulated,
            confluence_score: 0,
            outcome_at: None,
            entry_type: EntryType::Market,
            candle_close_price: 2000.0,
            live_market_price: None,
            retest_limit_price: None,
            data_source: "TEST".to_string(),
            is_entry_filled: true,
            ai_verdict: None,
            ai_confidence: None,
            ai_reason: None,
            delta_ratio: None,
            cvd: None,
            delta: None,
            rsi_value: None,
            rsi_state: None,
            rsi_divergence: None,
            macro_flow_alignment: None,
            macro_flow_note: None,
            cancel_reason: None,
            management_note: None,
        }
    }

    #[test]
    fn close_active_signals_only_closes_active_entries() {
        let now = Utc::now();
        let mut signals = vec![
            signal(SignalStatus::Active, false),
            signal(SignalStatus::Active, true),
            signal(SignalStatus::Target2Hit, false),
            signal(SignalStatus::Cancelled, false),
        ];

        assert_eq!(close_active_signals(&mut signals, now), 2);
        assert_eq!(signals[0].status, SignalStatus::Cancelled);
        assert_eq!(signals[1].status, SignalStatus::Cancelled);
        assert_eq!(signals[2].status, SignalStatus::Target2Hit);
        assert_eq!(signals[3].status, SignalStatus::Cancelled);
        assert_eq!(signals[0].outcome_at, Some(now));
    }
}
