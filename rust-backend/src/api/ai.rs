use super::AppState;
use crate::market_context::MarketContext;
use crate::models::Signal;
use crate::rsi::{divergence_label, rsi_snapshot};
use crate::signal_review::review_signal;
use axum::{extract::State, http::StatusCode, Json};
use serde_json::json;

/// POST /api/ai/review-signal — review thủ công từ dashboard.
/// Không gửi Telegram và không tiêu hao quota.
pub(super) async fn ai_review_signal(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    let config = state.config.read().await.clone();
    if !config.ai_signal_review_enabled {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "disabled",
                "message": "AI signal review đang TẮT (ai_signal_review_enabled=false)."
            })),
        );
    }

    let signal_value = payload
        .get("signal")
        .cloned()
        .unwrap_or_else(|| payload.clone());
    let mut signal: Signal = match serde_json::from_value(signal_value) {
        Ok(signal) => signal,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "message": format!("Signal JSON không hợp lệ: {}", err)
                })),
            )
        }
    };

    let mut context = match payload.get("context") {
        Some(value) if !value.is_null() => {
            match serde_json::from_value::<MarketContext>(value.clone()) {
                Ok(context) => Some(context),
                Err(err) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "status": "error",
                            "message": format!("Market context JSON không hợp lệ: {}", err)
                        })),
                    )
                }
            }
        }
        _ => state.market_context.read().await.clone(),
    };

    // Manual review có thể dùng signal cũ chưa có RSI. Luôn bổ sung RSI M15 live
    // từ nến hiện tại để AI có đủ 3 tầng, trừ khi caller đã truyền RSI sẵn.
    let live_rsi = rsi_snapshot(
        &state.candles.read().await.clone(),
        config.rsi_period,
        config.rsi_overbought,
        config.rsi_oversold,
        config.rsi_divergence_lookback,
        config.rsi_pivot_k,
    );
    if signal.rsi_value.is_none() {
        signal.rsi_value = live_rsi.value;
        signal.rsi_state = Some(live_rsi.state.clone());
        signal.rsi_divergence = Some(divergence_label(live_rsi.divergence));
    }
    if let Some(context) = context.as_mut() {
        if context.rsi_m15.is_none() {
            context.rsi_m15 = Some(live_rsi);
        }
    }
    let events = state.news_filter.read().await.calendar_events.clone();
    let macro_flow = state.macro_flow.read().await.clone();

    match review_signal(
        &signal,
        context.as_ref(),
        macro_flow.as_ref(),
        &config,
        &events,
    )
    .await
    {
        Some(review) => (
            StatusCode::OK,
            Json(json!({
                "status": "success",
                "review": review
            })),
        ),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "unavailable",
                "message": "AI risk review unavailable (thiếu OPENCODE_GO_API_KEY, timeout hoặc phản hồi không hợp lệ)."
            })),
        ),
    }
}
