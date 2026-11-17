use super::{current_live_view, current_scenarios, persist_state, AppState};
use crate::news_filter::{EconomicEvent, ImpactLevel};
use crate::scheduler::DailyScheduler;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use tracing::{info, warn};

fn event_status(
    event: &EconomicEvent,
    now: chrono::DateTime<Utc>,
    pre: i64,
    post: i64,
) -> &'static str {
    let start = event.scheduled_time - Duration::minutes(pre.max(0));
    let end = event.scheduled_time + Duration::minutes(post.max(0));
    if now >= start && now <= end {
        "ACTIVE"
    } else if end < now {
        "PAST"
    } else {
        "UPCOMING"
    }
}

fn next_high_impact_event(
    events: &[EconomicEvent],
    now: chrono::DateTime<Utc>,
) -> Option<&EconomicEvent> {
    events
        .iter()
        .filter(|event| {
            event.impact == ImpactLevel::High
                && event.currency.eq_ignore_ascii_case("USD")
                && event.scheduled_time > now
        })
        .min_by_key(|event| event.scheduled_time)
}

/// Lịch tin kinh tế + trạng thái Kill Switch đồng bộ với Frontend.
pub(super) async fn get_news_calendar(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let now = Utc::now();

    let (events, active, active_event, reason) = {
        let nf = state.news_filter.read().await;
        let (active, active_event, reason) = nf.evaluate_news_status(now, &config);
        (nf.calendar_events.clone(), active, active_event, reason)
    };

    let pre = config.pre_news_buffer_minutes;
    let post = config.post_news_buffer_minutes;
    let event_views: Vec<Value> = events
        .iter()
        .map(|event| {
            let mut value = serde_json::to_value(event).unwrap_or_else(|_| json!({}));
            value["status"] = json!(event_status(event, now, pre, post));
            value["minutesUntil"] = json!((event.scheduled_time - now).num_minutes());
            value
        })
        .collect();
    let next_high = next_high_impact_event(&events, now);
    let next_high_view = next_high.map(|event| {
        let mut value = serde_json::to_value(event).unwrap_or_else(|_| json!({}));
        value["minutesUntil"] = json!((event.scheduled_time - now).num_minutes());
        value
    });

    Json(json!({
        "events": event_views,
        "isKillSwitchActive": active,
        "activeEvent": active_event,
        "killSwitchReason": reason,
        "killSwitchEnabled": config.kill_switch_enabled,
        "macroBias": config.macro_bias,
        "nextHighImpactEvent": next_high_view,
        "nextHighImpactInMinutes": next_high.map(|event| (event.scheduled_time - now).num_minutes()),
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

    // 2. Báo cáo vĩ mô D1 + Macro/Flow (hoặc AI nếu có key). Không dùng M15
    // để tránh daily bias lệch với entry engine.
    let d1_candles = state.htf_d1_candles.read().await.clone();
    let report_candles = if d1_candles.len() >= 200 {
        d1_candles
    } else {
        state.candles.read().await.clone()
    };
    let live_price = *state.live_price.read().await;
    let macro_flow = state.macro_flow.read().await.clone();
    let mut report = scheduler
        .fetch_daily_macro_bias_from_context(&report_candles, live_price, macro_flow.as_ref())
        .await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 17, hour, minute, 0)
            .single()
            .expect("valid datetime")
    }

    fn event(
        title: &str,
        currency: &str,
        impact: ImpactLevel,
        scheduled_time: chrono::DateTime<Utc>,
    ) -> EconomicEvent {
        EconomicEvent {
            id: title.to_string(),
            title: title.to_string(),
            currency: currency.to_string(),
            impact,
            scheduled_time,
            forecast: None,
            previous: None,
            actual: None,
        }
    }

    #[test]
    fn event_status_marks_past_active_upcoming() {
        let now = at(12, 0);
        let active = event("Active", "USD", ImpactLevel::High, at(12, 10));
        let past = event("Past", "USD", ImpactLevel::High, at(10, 0));
        let upcoming = event("Upcoming", "USD", ImpactLevel::High, at(13, 0));

        assert_eq!(event_status(&active, now, 30, 30), "ACTIVE");
        assert_eq!(event_status(&past, now, 30, 30), "PAST");
        assert_eq!(event_status(&upcoming, now, 30, 30), "UPCOMING");
    }

    #[test]
    fn next_high_impact_skips_past_and_non_usd() {
        let now = at(12, 0);
        let events = vec![
            event("Past High", "USD", ImpactLevel::High, at(10, 0)),
            event("Future Medium", "USD", ImpactLevel::Medium, at(13, 0)),
            event("Future EUR High", "EUR", ImpactLevel::High, at(13, 30)),
            event("Future USD High", "USD", ImpactLevel::High, at(14, 0)),
        ];
        let next = next_high_impact_event(&events, now).expect("next high");
        assert_eq!(next.title, "Future USD High");
    }
}
