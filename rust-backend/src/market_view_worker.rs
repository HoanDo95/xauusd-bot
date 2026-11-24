//! DAILY MARKET VIEW Telegram worker.
//!
//! Sends exactly three discretionary plans per weekday (07:00, 13:00, 19:00
//! Vietnam time). Every daily send goes through the AI Market Strategist; if AI
//! is unavailable the deterministic plan is still sent with an explicit warning.
//! The worker never inserts a signal, consumes quota or places an order.

use crate::api::{compute_market_outlook, persist_state, AppState};
use crate::market_view::MarketOutlook;
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use std::collections::HashMap;
use std::time::Duration as StdDuration;
use tracing::{info, warn};

const MARKET_VIEW_INTERVAL_SECS: u64 = 60;
const DAILY_PLAN_HOURS_VN: [u32; 3] = [7, 13, 19];
const MIN_DAILY_CONFIDENCE: u8 = 60;
const DAILY_VIEW_RETENTION_DAYS: i64 = 7;

/// Return the most recent due slot that has not been sent yet, or `None`.
///
/// Example: if the bot starts at 14:00 VN it sends the 13:00 slot, not both
/// 07:00 and 13:00. A failed send remains due for a later retry in the same slot.
fn daily_plan_slot(
    vietnam_now: DateTime<Utc>,
    sent: &HashMap<String, DateTime<Utc>>,
) -> Option<u32> {
    if matches!(
        vietnam_now.weekday(),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    ) {
        return None;
    }
    let date_key = vietnam_now.format("%Y-%m-%d").to_string();
    let slot = DAILY_PLAN_HOURS_VN
        .iter()
        .copied()
        .filter(|hour| vietnam_now.hour() >= *hour)
        .max()?;
    let key = format!("{date_key}:{slot:02}");
    (!sent.contains_key(&key)).then_some(slot)
}

fn prune_daily_view_sent(sent: &mut HashMap<String, DateTime<Utc>>, now: DateTime<Utc>) {
    let cutoff = now - Duration::days(DAILY_VIEW_RETENTION_DAYS);
    sent.retain(|_, at| *at >= cutoff);
}

fn daily_plan_key(vietnam_now: DateTime<Utc>, slot: u32) -> String {
    format!("{}:{:02}", vietnam_now.format("%Y-%m-%d"), slot)
}

fn no_plan_reason(outlook: &MarketOutlook) -> String {
    let mut reason = match outlook.primary.as_ref() {
        None => "Chưa có PRIMARY.".to_string(),
        Some(primary) if primary.confidence < MIN_DAILY_CONFIDENCE => format!(
            "PRIMARY confidence {}% < {}%.",
            primary.confidence, MIN_DAILY_CONFIDENCE
        ),
        Some(_) => "Không đạt điều kiện gửi.".to_string(),
    };
    if let Some(note) = outlook
        .no_trade_reason
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        reason.push(' ');
        reason.push_str(note);
    }
    reason
}

pub fn spawn_market_view_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "🧭 [Market View] Worker đã khởi chạy (daily {:?} VN, min confidence {}%, AI mỗi lần gửi).",
            DAILY_PLAN_HOURS_VN, MIN_DAILY_CONFIDENCE
        );
        let mut interval = tokio::time::interval(StdDuration::from_secs(MARKET_VIEW_INTERVAL_SECS));

        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.is_active || !config.market_view_telegram_enabled {
                continue;
            }

            let now = Utc::now();
            let vietnam_now = now + Duration::hours(7);
            let sent = state.daily_view_sent.read().await.clone();
            let Some(slot) = daily_plan_slot(vietnam_now, &sent) else {
                continue;
            };

            let mut outlook = compute_market_outlook(&state).await;

            // Daily views must go through AI every time. If the provider is
            // unavailable, keep the deterministic plan and label it explicitly.
            let strategist = crate::api::refresh_market_strategist(&state, &outlook, &config).await;
            outlook.strategist = strategist;

            let telegram = state.telegram.read().await.clone();
            let extra_chat_ids = crate::telegram::parse_chat_ids(&config.telegram_alert_chat_ids);
            let result = match outlook.primary.as_ref() {
                Some(primary) if primary.confidence >= MIN_DAILY_CONFIDENCE => {
                    telegram
                        .send_daily_view_with_actionable_extra(&outlook, slot, &extra_chat_ids)
                        .await
                }
                _ => {
                    let reason = no_plan_reason(&outlook);
                    telegram.send_daily_view_no_plan(slot, &reason).await
                }
            };

            match result {
                Ok(msg_id) => {
                    let key = daily_plan_key(vietnam_now, slot);
                    {
                        let mut sent = state.daily_view_sent.write().await;
                        sent.insert(key, now);
                        prune_daily_view_sent(&mut sent, now);
                    }
                    if let Err(err) = persist_state(&state).await {
                        warn!("⚠️ [Market View] Không lưu được slot đã gửi: {}", err);
                    }
                    info!(
                        "🧭 [Market View] DAILY {:02}:00 VN · state {} · confidence {}% · ai={} · msg_id={}",
                        slot,
                        outlook.bot_state.as_str(),
                        outlook
                            .primary
                            .as_ref()
                            .map(|primary| primary.confidence)
                            .unwrap_or(0),
                        outlook.strategist.is_some(),
                        msg_id
                    );
                }
                Err(err) => warn!(
                    "⚠️ [Market View] Không gửi được Daily View {:02}:00: {}",
                    slot, err
                ),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn vn_at(y: i32, m: u32, d: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, hour, minute, 0)
            .single()
            .expect("valid Vietnam local time")
    }

    #[test]
    fn daily_slot_is_the_latest_due_slot() {
        assert_eq!(
            daily_plan_slot(vn_at(2026, 11, 16, 6, 59), &HashMap::new()),
            None
        );
        assert_eq!(
            daily_plan_slot(vn_at(2026, 11, 16, 7, 0), &HashMap::new()),
            Some(7)
        );
        assert_eq!(
            daily_plan_slot(vn_at(2026, 11, 16, 13, 0), &HashMap::new()),
            Some(13)
        );
        assert_eq!(
            daily_plan_slot(vn_at(2026, 11, 16, 19, 0), &HashMap::new()),
            Some(19)
        );
        assert_eq!(
            daily_plan_slot(vn_at(2026, 11, 16, 23, 0), &HashMap::new()),
            Some(19)
        );
    }

    #[test]
    fn sent_slot_is_not_sent_again() {
        let now = vn_at(2026, 11, 16, 13, 5);
        let mut sent = HashMap::new();
        sent.insert("2026-11-16:13".to_string(), now);
        assert_eq!(daily_plan_slot(now, &sent), None);
        assert_eq!(daily_plan_slot(vn_at(2026, 11, 16, 19, 1), &sent), Some(19));
    }

    #[test]
    fn weekends_have_no_daily_slot() {
        assert_eq!(
            daily_plan_slot(vn_at(2026, 11, 14, 13, 0), &HashMap::new()),
            None
        );
        assert_eq!(
            daily_plan_slot(vn_at(2026, 11, 15, 19, 0), &HashMap::new()),
            None
        );
    }

    #[test]
    fn prune_removes_old_slots() {
        let now = vn_at(2026, 11, 16, 13, 0);
        let mut sent = HashMap::new();
        sent.insert("2026-11-01:07".to_string(), now - Duration::days(15));
        sent.insert("2026-11-16:07".to_string(), now - Duration::hours(6));
        prune_daily_view_sent(&mut sent, now);
        assert!(!sent.contains_key("2026-11-01:07"));
        assert!(sent.contains_key("2026-11-16:07"));
    }
}
