use super::{persist_state, AppState};
use crate::models::BotConfig;
use crate::risk_manager::{PositionSizeResult, PositionSizer};
use crate::telegram::TelegramNotifier;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

/// Xóa toàn bộ secret trước khi trả config ra ngoài API.
fn redacted_config(mut config: BotConfig) -> BotConfig {
    config.telegram_bot_token.clear();
    config.webhook_passphrase.clear();
    config
}

/// Secret bị che hoặc để trống nghĩa là dashboard không có giá trị mới.
fn is_blank_or_masked(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed.chars().all(|ch| ch == '*' || ch == '•')
}

/// Giữ secret hiện tại khi payload gửi lên không chứa giá trị mới.
fn merge_secrets(mut incoming: BotConfig, current: &BotConfig) -> BotConfig {
    if is_blank_or_masked(&incoming.telegram_bot_token) {
        incoming.telegram_bot_token = current.telegram_bot_token.clone();
    }
    if incoming.telegram_chat_id.trim().is_empty() {
        incoming.telegram_chat_id = current.telegram_chat_id.clone();
    }
    if is_blank_or_masked(&incoming.webhook_passphrase) {
        incoming.webhook_passphrase = current.webhook_passphrase.clone();
    }
    incoming
}

/// Lấy cấu hình tham số bot hiện tại (không bao gồm secret).
pub(super) async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    Json(redacted_config(config))
}

/// Cập nhật cấu hình bot từ Dashboard và lưu bền vững xuống đĩa.
pub(super) async fn update_config(
    State(state): State<AppState>,
    Json(new_config): Json<BotConfig>,
) -> impl IntoResponse {
    let current_config = state.config.read().await.clone();
    let new_config = merge_secrets(new_config.sanitized(), &current_config);
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
            "config": redacted_config(new_config)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_secrets() -> BotConfig {
        BotConfig {
            telegram_bot_token: "123456:current-token".to_string(),
            telegram_chat_id: "-100987654".to_string(),
            webhook_passphrase: "current-passphrase".to_string(),
            symbol: "XAU/USD".to_string(),
            ..BotConfig::default()
        }
    }

    #[test]
    fn redacted_config_clears_secrets_but_keeps_other_fields() {
        let redacted = redacted_config(config_with_secrets());

        assert!(redacted.telegram_bot_token.is_empty());
        assert!(redacted.webhook_passphrase.is_empty());
        assert_eq!(redacted.telegram_chat_id, "-100987654");
        assert_eq!(redacted.symbol, "XAU/USD");
    }

    #[test]
    fn merge_secrets_keeps_current_when_incoming_blank() {
        let current = config_with_secrets();
        let mut incoming = config_with_secrets();
        incoming.telegram_bot_token.clear();
        incoming.webhook_passphrase = "   ".to_string();

        let merged = merge_secrets(incoming, &current);

        assert_eq!(merged.telegram_bot_token, "123456:current-token");
        assert_eq!(merged.webhook_passphrase, "current-passphrase");
    }

    #[test]
    fn merge_secrets_keeps_current_when_incoming_masked() {
        let current = config_with_secrets();
        let mut incoming = config_with_secrets();
        incoming.telegram_bot_token = "********".to_string();
        incoming.webhook_passphrase = "••••••••".to_string();

        let merged = merge_secrets(incoming, &current);

        assert_eq!(merged.telegram_bot_token, "123456:current-token");
        assert_eq!(merged.webhook_passphrase, "current-passphrase");
    }

    #[test]
    fn merge_secrets_accepts_new_non_empty_values() {
        let current = config_with_secrets();
        let mut incoming = config_with_secrets();
        incoming.telegram_bot_token = "999999:new-token".to_string();
        incoming.webhook_passphrase = "new-passphrase".to_string();

        let merged = merge_secrets(incoming, &current);

        assert_eq!(merged.telegram_bot_token, "999999:new-token");
        assert_eq!(merged.webhook_passphrase, "new-passphrase");
    }

    #[test]
    fn merge_secrets_preserves_chat_id_when_incoming_blank() {
        let current = config_with_secrets();
        let mut incoming = config_with_secrets();
        incoming.telegram_chat_id = "  ".to_string();

        let merged = merge_secrets(incoming, &current);

        assert_eq!(merged.telegram_chat_id, "-100987654");
    }
}
