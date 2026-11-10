use super::{persist_state, AppState};
use crate::models::BotConfig;
use crate::risk_manager::{PositionSizeResult, PositionSizer};
use crate::telegram::TelegramNotifier;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

/// Lấy cấu hình tham số bot hiện tại
pub(super) async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await;
    Json((*config).clone())
}

/// Cập nhật cấu hình bot từ Dashboard và lưu bền vững xuống đĩa.
pub(super) async fn update_config(
    State(state): State<AppState>,
    Json(new_config): Json<BotConfig>,
) -> impl IntoResponse {
    let new_config = new_config.sanitized();
    {
        let mut config = state.config.write().await;
        *config = new_config.clone();
    }
    {
        let mut telegram = state.telegram.write().await;
        *telegram = TelegramNotifier::new(
            new_config.telegram_bot_token.clone(),
            new_config.telegram_chat_id.clone(),
        );
    }

    if let Err(err) = persist_state(&state).await {
        warn!("⚠️ Không lưu được config xuống đĩa: {}", err);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "message": format!("Cấu hình đã cập nhật trong RAM nhưng ghi đĩa thất bại: {}", err)
            })),
        );
    }

    info!("⚙️ Cấu hình Bot đã được cập nhật và lưu xuống đĩa!");
    (
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "message": "Đã lưu thông số cấu hình, quota Sniper và Telegram credentials.",
            "config": new_config
        })),
    )
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct CalculateLotRequest {
    #[serde(default)]
    account_balance: Option<f64>,
    #[serde(default)]
    risk_percentage: Option<f64>,
    #[serde(default)]
    sl_distance: Option<f64>,
    #[serde(default)]
    tp1_distance: Option<f64>,
    #[serde(default)]
    tp2_distance: Option<f64>,
    #[serde(default)]
    current_price: Option<f64>,
    #[serde(default)]
    leverage: Option<f64>,
}

/// Giá mặc định cho calculator: ưu tiên live price, sau đó giá đóng nến gần nhất.
pub(super) fn default_current_price(
    live_price: Option<f64>,
    last_closed_price: Option<f64>,
) -> f64 {
    live_price
        .filter(|price| *price > 0.0)
        .or(last_closed_price)
        .unwrap_or(2684.50)
}

/// Response song ngữ snake_case (Rust/legacy) + camelCase (dashboard `src/lib/api.ts`).
pub(super) fn calculate_lot_response(result: &PositionSizeResult) -> serde_json::Value {
    json!({
        "lot_size": result.lot_size,
        "risk_amount_usd": result.risk_amount_usd,
        "potential_profit_tp1_usd": result.potential_profit_tp1_usd,
        "potential_profit_tp2_usd": result.potential_profit_tp2_usd,
        "estimated_margin_usd": result.estimated_margin_usd,
        "risk_assessment": result.risk_assessment,
        "lotSize": result.lot_size,
        "riskAmountUsd": result.risk_amount_usd,
        "potentialProfitTp1Usd": result.potential_profit_tp1_usd,
        "potentialProfitTp2Usd": result.potential_profit_tp2_usd,
        "estimatedMarginUsd": result.estimated_margin_usd,
        "riskAssessment": result.risk_assessment,
    })
}

/// Tính Lot Size trực tiếp từ PositionSizer (Rust) cho Dashboard.
pub(super) async fn calculate_lot(
    State(state): State<AppState>,
    Json(payload): Json<CalculateLotRequest>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let live_price = *state.live_price.read().await;
    let last_closed_price = state.candles.read().await.last().map(|candle| candle.close);
    let default_price = default_current_price(live_price, last_closed_price);

    let balance = payload.account_balance.unwrap_or(config.account_balance);
    let risk_pct = payload.risk_percentage.unwrap_or(config.risk_percentage);
    let sl = payload.sl_distance.unwrap_or(9.5);
    let tp1 = payload.tp1_distance.unwrap_or(sl * 2.0);
    let tp2 = payload.tp2_distance.unwrap_or(sl * 3.0);
    let current_price = payload.current_price.unwrap_or(default_price);
    let leverage = payload.leverage.unwrap_or(config.account_leverage);

    let result =
        PositionSizer::calculate_lot_size(balance, risk_pct, sl, tp1, tp2, current_price, leverage);

    Json(calculate_lot_response(&result))
}
