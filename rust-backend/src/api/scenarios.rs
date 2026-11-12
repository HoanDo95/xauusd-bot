use super::AppState;
use crate::scenarios::{build_scenarios, ScenarioResponse};
use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;

/// GET /api/scenarios — four read-only reference scenarios for the current market.
///
/// This handler never mutates state, sends Telegram, inserts/persists a signal or
/// consumes the daily Sniper quota. It also never runs the strict signal engine,
/// so the scenarios remain available while `/api/live-view` reports `no_setup`.
pub(super) async fn get_scenarios(State(state): State<AppState>) -> impl IntoResponse {
    Json(current_scenarios(&state).await)
}

/// Shared read-only scenario builder used by `/api/scenarios`, scanner start and
/// the manual scenario-Telegram handler. No side effects.
pub(crate) async fn current_scenarios(state: &AppState) -> ScenarioResponse {
    let config = state.config.read().await.clone();
    let candles = state.candles.read().await.clone();
    let live_price = *state.live_price.read().await;
    let market_context = state.market_context.read().await.clone();
    let macro_flow = state.macro_flow.read().await.clone();
    let active_data_source = state.active_data_source.read().await.clone();
    let active_volume_source = state.active_volume_source.read().await.clone();
    let now_ms = Utc::now().timestamp_millis();

    build_scenarios(
        &candles,
        &config,
        live_price,
        market_context.as_ref(),
        macro_flow.as_ref(),
        &active_data_source,
        &active_volume_source,
        now_ms,
    )
}
