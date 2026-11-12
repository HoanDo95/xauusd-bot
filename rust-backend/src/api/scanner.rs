use super::live::current_live_view;
use super::scenarios::current_scenarios;
use super::{persist_state, AppState};
use crate::ai_decision::{
    ai_decision_enabled, relaxed_candidate_config, review_market_decision, AiDecision,
};
use crate::analyzer::GoldMarketAnalyzer;
use crate::models::BotConfig;
use crate::rsi::rsi_snapshot;
use crate::scenarios::ScenarioResponse;
use crate::volume_delta::compute_delta;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde_json::{json, Value};
use tracing::{info, warn};

const TELEGRAM_ERROR_MESSAGE: &str =
    "Không gửi được Telegram. Kiểm tra token/chat ID và log server.";
const TELEGRAM_NOT_CONFIGURED_MESSAGE: &str =
    "Telegram chưa cấu hình token/chat ID — scenario view chỉ hiển thị trên dashboard.";

/// Telegram simulator returns Ok(1001) when token/chat is empty, so the API
/// must check configuration explicitly before reporting `telegramSent=true`.
fn telegram_configured(config: &BotConfig) -> bool {
    !config.telegram_bot_token.trim().is_empty() && !config.telegram_chat_id.trim().is_empty()
}

/// Pure scanner-state transition. Start/stop must only change `is_active`; no
/// quota, signal or trade-state field is touched.
pub(super) fn apply_scanner_state(mut config: BotConfig, active: bool) -> BotConfig {
    config.is_active = active;
    config
}

/// Result shared by the HTTP Start handler and the Telegram `/start` command.
pub(crate) struct StartResult {
    pub was_active: bool,
    pub telegram_sent: bool,
    pub telegram_error: Option<String>,
    pub live_view: Value,
    pub scenarios: ScenarioResponse,
    pub ai_decision: Option<AiDecision>,
    pub ai_error: Option<String>,
}

const AI_DECISION_UNAVAILABLE_MESSAGE: &str =
    "AI Decision không khả dụng (thiếu OPENCODE_GO_API_KEY, lỗi provider hoặc phản hồi không hợp lệ). Không có tín hiệu AI mới; scenario view vẫn được gửi.";

/// Run one AI decision for the Start brief. Read-only: never inserts a signal
/// and never consumes the Sniper quota.
async fn start_ai_decision(
    state: &AppState,
    config: &BotConfig,
    scenarios: &ScenarioResponse,
) -> (Option<AiDecision>, Option<String>) {
    if !ai_decision_enabled(config) {
        return (None, None);
    }

    let candles = state.candles.read().await.clone();
    let live_price = *state.live_price.read().await;
    let mut context = state.market_context.read().await.clone();
    let macro_flow = state.macro_flow.read().await.clone();
    let events = state.news_filter.read().await.calendar_events.clone();
    let now = Utc::now();
    let now_ms = now.timestamp_millis();

    let rsi = rsi_snapshot(
        &candles,
        config.rsi_period,
        config.rsi_overbought,
        config.rsi_oversold,
        config.rsi_divergence_lookback,
        config.rsi_pivot_k,
    );
    if let Some(context) = context.as_mut() {
        context.rsi_m15 = Some(rsi.clone());
    }
    let delta = compute_delta(&candles, config.volume_delta_lookback);
    let relaxed = relaxed_candidate_config(config);
    let candidate =
        GoldMarketAnalyzer::analyze_market_with_live_price(&candles, &relaxed, live_price, now_ms);

    let decision = review_market_decision(
        candidate.as_ref(),
        context.as_ref(),
        macro_flow.as_ref(),
        Some(scenarios),
        Some(&rsi),
        Some(&delta),
        &events,
        config,
    )
    .await;

    match decision {
        Some(decision) => {
            *state.last_ai_decision.write().await = Some(decision.clone());
            (Some(decision), None)
        }
        None => {
            warn!(
                "⚠️ [AI Decision] Start không có quyết định AI — fail-closed cho tín hiệu, gửi scenario view."
            );
            (None, Some(AI_DECISION_UNAVAILABLE_MESSAGE.to_string()))
        }
    }
}

/// Shared Start logic: flip `is_active`, persist, build the read-only views and
/// send the reference scenario summary. Never inserts a signal or spends quota.
pub(crate) async fn start_scanner_inner(state: &AppState) -> StartResult {
    let was_active = {
        let mut config = state.config.write().await;
        let was_active = config.is_active;
        *config = apply_scanner_state(config.clone(), true);
        was_active
    };

    if was_active {
        info!("▶️ Scanner đã bật sẵn — gửi lại scenario view theo yêu cầu.");
    } else {
        info!("▶️ Scanner đã bật — gửi scenario view tham khảo tới Telegram.");
    }

    if let Err(err) = persist_state(state).await {
        warn!("⚠️ Không lưu được trạng thái scanner start: {}", err);
    }

    let live_view = current_live_view(state).await;
    let scenarios = current_scenarios(state).await;
    let config = state.config.read().await.clone();
    let (ai_decision, ai_error) = start_ai_decision(state, &config, &scenarios).await;

    let (telegram_sent, telegram_error) = if !telegram_configured(&config) {
        warn!("⚠️ Scanner start: Telegram chưa cấu hình token/chat ID.");
        (false, Some(TELEGRAM_NOT_CONFIGURED_MESSAGE.to_string()))
    } else {
        let telegram = state.telegram.read().await.clone();
        let send_result = match ai_decision.as_ref() {
            Some(decision) => {
                telegram
                    .send_ai_market_brief(decision, &scenarios, &live_view)
                    .await
            }
            None => telegram.send_scenario_summary(&live_view, &scenarios).await,
        };
        match send_result {
            Ok(_) => (true, None),
            Err(err) => {
                warn!("⚠️ Không gửi được brief tới Telegram: {}", err);
                (false, Some(TELEGRAM_ERROR_MESSAGE.to_string()))
            }
        }
    };

    StartResult {
        was_active,
        telegram_sent,
        telegram_error,
        live_view,
        scenarios,
        ai_decision,
        ai_error,
    }
}

/// POST /api/scanner/start — enable the scanner and push the reference scenario
/// view to Telegram (the explicit user action that produced the Start click).
///
/// It never inserts a signal or consumes the Sniper quota. If the scanner is
/// already active, the scenario view is still sent so Start always gives a view,
/// but the transition is not logged as a new start.
pub(super) async fn start_scanner(State(state): State<AppState>) -> impl IntoResponse {
    let result = start_scanner_inner(&state).await;

    let mut payload = json!({
        "status": "started",
        "telegramSent": result.telegram_sent,
        "liveView": result.live_view,
        "scenarios": result.scenarios,
        "aiDecision": result.ai_decision,
        "aiError": result.ai_error,
    });
    if let Some(error) = result.telegram_error {
        payload["telegramError"] = json!(error);
    }

    (StatusCode::OK, Json(payload))
}

/// Shared Stop logic: only `is_active` changes; open signals and quota counters
/// stay untouched. Returns an error only when the state cannot be persisted.
pub(crate) async fn stop_scanner_inner(state: &AppState) -> Result<(), String> {
    {
        let mut config = state.config.write().await;
        *config = apply_scanner_state(config.clone(), false);
    }

    persist_state(state).await?;
    info!("⏸ Scanner đã tắt.");
    Ok(())
}

/// POST /api/scanner/stop — pause the scanner. Only `is_active` changes; open
/// signals and quota counters stay untouched.
pub(super) async fn stop_scanner(State(state): State<AppState>) -> impl IntoResponse {
    match stop_scanner_inner(&state).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "stopped" }))),
        Err(err) => {
            warn!("⚠️ Không lưu được trạng thái scanner stop: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "error",
                    "message": "Scanner đã dừng trong RAM nhưng ghi trạng thái xuống đĩa thất bại."
                })),
            )
        }
    }
}

/// POST /api/scenarios/telegram — manual "Gửi Telegram" button on the Scenario
/// View card. Sends the same reference snapshot; no quota, signal or persistence.
pub(super) async fn send_scenario_telegram(State(state): State<AppState>) -> impl IntoResponse {
    let live_view = current_live_view(&state).await;
    let scenarios = current_scenarios(&state).await;
    let config = state.config.read().await.clone();

    if !telegram_configured(&config) {
        warn!("⚠️ Gửi scenario Telegram: token/chat ID chưa cấu hình.");
        return (
            StatusCode::OK,
            Json(json!({
                "status": "error",
                "telegramSent": false,
                "message": TELEGRAM_NOT_CONFIGURED_MESSAGE
            })),
        );
    }

    let telegram = state.telegram.read().await.clone();
    match telegram.send_scenario_summary(&live_view, &scenarios).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "status": "sent",
                "telegramSent": true
            })),
        ),
        Err(err) => {
            warn!("⚠️ Gửi scenario Telegram thất bại: {}", err);
            (
                StatusCode::OK,
                Json(json!({
                    "status": "error",
                    "telegramSent": false,
                    "message": TELEGRAM_ERROR_MESSAGE
                })),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> BotConfig {
        BotConfig {
            is_active: false,
            max_signals_per_day: 3,
            telegram_chat_id: "-100987654".to_string(),
            symbol: "XAU/USD".to_string(),
            ..BotConfig::default()
        }
    }

    #[test]
    fn telegram_configured_requires_both_token_and_chat_id() {
        let mut cfg = config();
        cfg.telegram_bot_token = "123:abc".to_string();
        assert!(telegram_configured(&cfg));

        cfg.telegram_bot_token.clear();
        assert!(!telegram_configured(&cfg));

        cfg.telegram_bot_token = "123:abc".to_string();
        cfg.telegram_chat_id.clear();
        assert!(!telegram_configured(&cfg));
    }

    #[test]
    fn apply_scanner_state_only_toggles_is_active() {
        let base = config();
        let started = apply_scanner_state(base.clone(), true);

        assert!(started.is_active);
        assert_eq!(started.max_signals_per_day, base.max_signals_per_day);
        assert_eq!(started.telegram_chat_id, base.telegram_chat_id);
        assert_eq!(started.symbol, base.symbol);
        assert_eq!(started.min_risk_reward, base.min_risk_reward);

        let stopped = apply_scanner_state(started, false);
        assert!(!stopped.is_active);
        assert_eq!(stopped.max_signals_per_day, base.max_signals_per_day);
        assert_eq!(stopped.telegram_chat_id, base.telegram_chat_id);
        assert_eq!(stopped.symbol, base.symbol);
    }
}
