use super::AppState;
use crate::scenarios::{build_scenarios, ScenarioResponse};
use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;
use tracing::warn;

/// GET /api/scenarios — four read-only reference scenarios for the current market.
///
/// This handler never sends Telegram, inserts/persists a signal or consumes the
/// daily Sniper quota. It may freeze the daily plan for the current slot. It also
/// never runs the strict signal engine, so the scenarios remain available while
/// `/api/live-view` reports `no_setup`.
pub(super) async fn get_scenarios(State(state): State<AppState>) -> impl IntoResponse {
    Json(current_scenarios(&state).await)
}

/// Shared scenario builder used by `/api/scenarios`, the scanner and the manual
/// scenario-Telegram handler. The first call of a daily slot freezes the plan
/// (persisted to `data/daily_plan.json`); later calls return the same levels.
/// No Telegram/signal/quota side effects.
pub(crate) async fn current_scenarios(state: &AppState) -> ScenarioResponse {
    let now = Utc::now();
    let slot = crate::daily_plan::current_slot(now);
    let live_price = *state.live_price.read().await;

    // Return the frozen plan for the current slot. A dashboard refresh must not
    // re-anchor the levels to the latest price; only a new slot rebuilds.
    {
        let plan = state.daily_plan.read().await;
        if let Some(plan) = plan.as_ref() {
            if plan.slot_key == slot.key {
                let mut response = plan.scenarios.clone();
                if let Some(price) = live_price.filter(|value| value.is_finite() && *value > 0.0) {
                    response.price = Some(price);
                }
                return response;
            }
        }
    }

    let config = state.config.read().await.clone();
    let candles = state.candles.read().await.clone();
    let market_context = state.market_context.read().await.clone();
    let macro_flow = state.macro_flow.read().await.clone();
    let active_data_source = state.active_data_source.read().await.clone();
    let active_volume_source = state.active_volume_source.read().await.clone();
    let now_ms = now.timestamp_millis();

    let mut scenarios = build_scenarios(
        &candles,
        &config,
        live_price,
        market_context.as_ref(),
        macro_flow.as_ref(),
        &active_data_source,
        &active_volume_source,
        now_ms,
    );

    if scenarios.status == "ok" {
        scenarios.generated_at = slot.generated_at.to_rfc3339();
        let plan = crate::daily_plan::DailyPlan {
            slot_key: slot.key.clone(),
            generated_at: slot.generated_at,
            valid_until: slot.valid_until,
            scenarios: scenarios.clone(),
        };
        if let Err(err) = state.daily_plan_store.save(&plan) {
            warn!("⚠️ [DailyPlan] Không lưu được plan: {}", err);
        }
        *state.daily_plan.write().await = Some(plan);
    }

    scenarios
}
