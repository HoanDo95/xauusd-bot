use super::{current_live_view, current_scenarios, persist_state, AppState};
use crate::news_filter::ImpactLevel;
use crate::scheduler::DailyScheduler;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde_json::json;
use tracing::{info, warn};

/// Lịch tin kinh tế + trạng thái Kill Switch đồng bộ với Frontend.
pub(super) async fn get_news_calendar(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let now = Utc::now();

    let (events, active, active_event, reason) = {
        let nf = state.news_filter.read().await;
        let (active, active_event, reason) = nf.evaluate_news_status(now, &config);
        (nf.calendar_events.clone(), active, active_event, reason)
    };

    Json(json!({
        "events": events,
        "isKillSwitchActive": active,
        "activeEvent": active_event,
        "killSwitchReason": reason,
        "killSwitchEnabled": config.kill_switch_enabled,
        "macroBias": config.macro_bias
    }))
}

/// Lấy thông tin Macro Bias mới nhất trong ngày.
pub(super) async fn get_latest_macro_bias(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let report = state.daily_report.read().await.clone();
    let now = Utc::now();

    let (is_kill_switch_active, active_event) = {
        let nf = state.news_filter.read().await;
        let (active, event, _) = nf.evaluate_news_status(now, &config);
        (active, event)
    };

    Json(json!({
        "macro_bias": config.macro_bias,
        "daily_report": report,
        "is_kill_switch_active": is_kill_switch_active,
        "active_blocking_event": active_event,
        "kill_switch_enabled": config.kill_switch_enabled
    }))
}

/// Kích hoạt phiên chạy Daily Cron thủ công (07:00 AM Scan).
pub(super) async fn trigger_daily_macro_cron(State(state): State<AppState>) -> impl IntoResponse {
    info!("⚡ [Manual Trigger] Kích hoạt phiên quét Daily Cron 07:00 AM...");

    // AI là tùy chọn; from_env tự chọn OpenCode Go/Gemini/OpenRouter hoặc EMA fallback.
    let scheduler = DailyScheduler::from_env();

    // 1. Quét lịch tin ForexFactory thật
    let events = scheduler.fetch_daily_economic_events().await;
    {
        let mut nf = state.news_filter.write().await;
        nf.update_events(events.clone());
    }

    // 2. Báo cáo vĩ mô định lượng EMA (miễn phí) hoặc AI nếu có key
    let candles = state.candles.read().await.clone();
    let mut report = if candles.is_empty() {
        scheduler.fetch_daily_macro_bias(2684.50).await
    } else {
        scheduler
            .fetch_daily_macro_bias_from_candles(&candles)
            .await
    };
    report.high_impact_events_today = events
        .iter()
        .filter(|event| event.impact == ImpactLevel::High)
        .take(6)
        .map(|event| {
            format!(
                "{} ({})",
                event.title,
                event.scheduled_time.format("%d/%m %H:%M UTC")
            )
        })
        .collect();

    // 3. Chỉ cập nhật daily_report để hiển thị. KHÔNG ghi đè config.macro_bias:
    // bias báo cáo là tham khảo; AUTO phải giữ nguyên để EMA50/200 chạy động.
    info!(
        "🤖 [Manual Trigger] Nguồn báo cáo: {} | bias tư vấn: {} (config.macro_bias giữ nguyên).",
        scheduler.report_source(),
        report.macro_bias.as_str()
    );
    {
        let mut rep_lock = state.daily_report.write().await;
        *rep_lock = Some(report.clone());
    }
    if let Err(err) = persist_state(&state).await {
        warn!("⚠️ Không lưu được Daily Macro report: {}", err);
    }

    info!("✅ [Manual Trigger] Đã hoàn tất phiên quét Daily Cron!");

    (
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "message": "Đã chạy thành công phiên quét 07:00 AM: Cập nhật Lịch tin ForexFactory & Nhận định vĩ mô EMA!",
            "report": report,
            "events_synced": events.len()
        })),
    )
}

/// POST /api/daily-plan/telegram — gửi lại kế hoạch ngày 07:00 theo yêu cầu.
/// Read-only: không quota, không chèn signal, không đổi scanner.
pub(super) async fn send_daily_plan_telegram(State(state): State<AppState>) -> impl IntoResponse {
    let report = state.daily_report.read().await.clone();
    let Some(report) = report else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "unavailable",
                "message": "Daily macro report chưa sẵn sàng. Gọi /api/cron/trigger-daily-macro trước."
            })),
        );
    };

    let live_view = current_live_view(&state).await;
    let scenarios = current_scenarios(&state).await;
    let events = state.news_filter.read().await.calendar_events.clone();
    let telegram = state.telegram.read().await.clone();

    match telegram
        .send_daily_plan(&live_view, &scenarios, &events, &report)
        .await
    {
        Ok(message_id) => (
            StatusCode::OK,
            Json(json!({
                "status": "sent",
                "messageId": message_id
            })),
        ),
        Err(err) => {
            warn!("⚠️ Gửi daily plan Telegram thất bại: {}", err);
            (
                StatusCode::OK,
                Json(json!({
                    "status": "error",
                    "message": "Không gửi được daily plan. Kiểm tra token/chat ID và log server."
                })),
            )
        }
    }
}
