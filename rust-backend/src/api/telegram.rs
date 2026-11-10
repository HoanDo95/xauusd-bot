use super::AppState;
use crate::models::Signal;
use crate::telegram::TelegramNotifier;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Default, Deserialize)]
pub(super) struct TelegramTestPayload {
    #[serde(default)]
    token: Option<String>,
    #[serde(default, alias = "chatId")]
    chat_id: Option<String>,
}

/// Test gửi tin nhắn Telegram từ dashboard (dùng token/chatId gửi lên hoặc config).
pub(super) async fn trigger_test_telegram(
    State(state): State<AppState>,
    payload: Option<Json<TelegramTestPayload>>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let request = payload.map(|Json(value)| value).unwrap_or_default();

    let token = request
        .token
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.telegram_bot_token.clone());
    let chat_id = request
        .chat_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.telegram_chat_id.clone());

    if token.trim().is_empty() || chat_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "message": "Vui lòng cung cấp Telegram Bot Token và Chat ID trước khi test!"
            })),
        );
    }

    let notifier = TelegramNotifier::new(token, chat_id);
    match notifier.send_test_ping().await {
        Ok(message) => (
            StatusCode::OK,
            Json(json!({ "status": "success", "message": message })),
        ),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "message": err })),
        ),
    }
}

/// Gửi một tín hiệu cụ thể lên Telegram (nút "View Telegram Alert" trên Dashboard).
/// Chấp nhận cả payload `{ signal, token, chatId }` và raw Signal JSON.
pub(super) async fn send_signal_telegram(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let signal_value = payload
        .get("signal")
        .cloned()
        .unwrap_or_else(|| payload.clone());

    let signal: Signal = match serde_json::from_value(signal_value) {
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

    let config = state.config.read().await.clone();
    let token = payload
        .get("token")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.telegram_bot_token)
        .to_string();
    let chat_id = payload
        .get("chatId")
        .or_else(|| payload.get("chat_id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&config.telegram_chat_id)
        .to_string();

    let notifier = if token == config.telegram_bot_token && chat_id == config.telegram_chat_id {
        state.telegram.read().await.clone()
    } else {
        TelegramNotifier::new(token, chat_id)
    };

    match notifier.send_signal_alert(&signal).await {
        Ok(message_id) => (
            StatusCode::OK,
            Json(json!({
                "status": "success",
                "messageId": message_id
            })),
        ),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "message": err
            })),
        ),
    }
}
