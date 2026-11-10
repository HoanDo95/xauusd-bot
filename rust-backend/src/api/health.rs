use super::{price_source_label, AppState};
use crate::storage::PersistentStore;
use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;
use serde_json::json;

pub(super) async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let signals = state.signals.read().await.clone();
    let candles_count = state.candles.read().await.len();
    let live_price = *state.live_price.read().await;
    let active_data_source = state.active_data_source.read().await.clone();
    let spot_price = *state.spot_price.read().await;
    let spot_basis = *state.spot_basis.read().await;
    let price_source = price_source_label(&active_data_source, spot_basis);
    let now = Utc::now();

    let signals_today = PersistentStore::count_live_signals_today(&signals, now);
    let daily_quota_reached =
        PersistentStore::has_reached_daily_quota(&signals, config.max_signals_per_day, now);
    let is_market_safe = state
        .news_filter
        .read()
        .await
        .check_kill_switch(now, &config);

    Json(json!({
        "status": "healthy",
        "service": "Gold Signal Bot (Rust Backend)",
        "engine": "Tokio 1.40 / Axum 0.7 High Performance",
        "asset": "XAU/USD",
        "version": "1.3.0",
        "symbol": config.symbol,
        "timeframe": config.timeframe,
        "isActive": config.is_active,
        "candlesCount": candles_count,
        "signalsCount": signals.len(),
        "signalsToday": signals_today,
        "maxSignalsPerDay": config.max_signals_per_day,
        "dailyQuotaReached": daily_quota_reached,
        "sessionFilterEnabled": config.session_filter_enabled,
        "h4FilterEnabled": config.htf_h4_filter_enabled,
        "d1FilterEnabled": config.htf_d1_filter_enabled,
        "aiReviewEnabled": config.ai_signal_review_enabled,
        "volumeDeltaFilterEnabled": config.volume_delta_filter_enabled,
        "rsiFilterEnabled": config.rsi_filter_enabled,
        "dataSource": active_data_source,
        "priceSource": price_source,
        "spotPrice": spot_price,
        "spotBasis": spot_basis,
        "livePrice": live_price,
        "killSwitchActive": !is_market_safe,
        "cron_schedule": "07:00 AM Daily",
        "tradingview_webhook": "/api/webhook/tradingview"
    }))
}
