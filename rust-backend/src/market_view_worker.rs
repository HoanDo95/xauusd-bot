//! MARKET VIEW Telegram worker.
//!
//! The dashboard/API computes the view 24/5. This worker sends:
//! - one daily plan at/after 13:00 Vietnam time on weekdays, even when the view
//!   is unchanged;
//! - an extra message on a new H1 close when the view changed materially and the
//!   cooldown elapsed.
//!
//! It never inserts a signal, consumes quota or places an order.

use crate::api::{compute_market_outlook, AppState};
use crate::market_view::MarketOutlook;
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use std::time::Duration as StdDuration;
use tracing::{info, warn};

const MARKET_VIEW_INTERVAL_SECS: u64 = 60;
const DAILY_PLAN_HOUR_VN: u32 = 13;

/// A weekday daily plan is due at/after 13:00 Vietnam time, once per date.
fn daily_plan_due(vietnam_now: DateTime<Utc>, last_date: Option<chrono::NaiveDate>) -> bool {
    let weekday = !matches!(
        vietnam_now.weekday(),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    );
    weekday
        && vietnam_now.hour() >= DAILY_PLAN_HOUR_VN
        && last_date != Some(vietnam_now.date_naive())
}

pub fn spawn_market_view_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "🧭 [Market View] Worker đã khởi chạy (chu kỳ {} giây, daily plan {}:00 VN)...",
            MARKET_VIEW_INTERVAL_SECS, DAILY_PLAN_HOUR_VN
        );
        let mut interval = tokio::time::interval(StdDuration::from_secs(MARKET_VIEW_INTERVAL_SECS));
        let mut last_h1_timestamp: Option<i64> = None;
        let mut last_key: Option<String> = None;
        let mut last_sent: Option<DateTime<Utc>> = None;
        let mut last_daily_plan_date: Option<chrono::NaiveDate> = None;

        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.is_active {
                continue;
            }
            if !config.market_view_telegram_enabled && !config.market_strategist_enabled {
                continue;
            }

            let now = Utc::now();
            let vietnam_now = now + Duration::hours(7);
            let today = vietnam_now.date_naive();
            let daily_due = daily_plan_due(vietnam_now, last_daily_plan_date);

            let h1_timestamp = {
                let context = state.market_context.read().await;
                context
                    .as_ref()
                    .map(|context| context.h1.last_candle_timestamp)
                    .unwrap_or(0)
            };
            let h1_changed = h1_timestamp > 0 && last_h1_timestamp != Some(h1_timestamp);
            if !h1_changed && !daily_due {
                continue;
            }
            if h1_changed {
                last_h1_timestamp = Some(h1_timestamp);
            }

            let mut outlook = compute_market_outlook(&state).await;
            let strategist = if config.market_strategist_enabled {
                crate::api::refresh_market_strategist(&state, &outlook, &config).await
            } else {
                *state.market_strategist_view.write().await = None;
                None
            };
            outlook.strategist = strategist;

            if !config.market_view_telegram_enabled {
                if daily_due {
                    last_daily_plan_date = Some(today);
                }
                continue;
            }

            let key = outlook_key(&outlook);
            let cooldown = Duration::minutes(config.market_view_cooldown_minutes.max(15));
            let cooldown_elapsed = match last_sent {
                None => true,
                Some(sent) => now - sent >= cooldown,
            };
            let should_send =
                daily_due || (last_key.as_deref() != Some(key.as_str()) && cooldown_elapsed);
            if !should_send {
                continue;
            }

            let telegram = state.telegram.read().await.clone();
            match telegram.send_market_view(&outlook).await {
                Ok(msg_id) => {
                    info!(
                        "🧭 [Market View] {} · H1 {} · state {} · msg_id={}",
                        if daily_due { "DAILY PLAN" } else { "H1 change" },
                        h1_timestamp,
                        outlook.bot_state.as_str(),
                        msg_id
                    );
                    if daily_due {
                        last_daily_plan_date = Some(today);
                    }
                    last_key = Some(key);
                    last_sent = Some(now);
                }
                Err(err) => warn!("⚠️ [Market View] Không gửi được Telegram: {}", err),
            }
        }
    });
}

fn outlook_key(outlook: &MarketOutlook) -> String {
    let primary = outlook
        .primary
        .as_ref()
        .map(|primary| {
            format!(
                "{}:{:.2}:{:.2}",
                primary.scenario_id, primary.zone_low, primary.zone_high
            )
        })
        .unwrap_or_default();
    let alternative = outlook
        .alternative
        .as_ref()
        .map(|alternative| alternative.invalidation.clone())
        .unwrap_or_default();
    let strategist = outlook
        .strategist
        .as_ref()
        .map(|view| {
            format!(
                "{}:{}:{}",
                view.primary_scenario_id.as_deref().unwrap_or(""),
                view.primary_action.as_deref().unwrap_or(""),
                view.market_view
            )
        })
        .unwrap_or_default();
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}",
        outlook.bot_state.as_str(),
        outlook.structure,
        primary,
        alternative,
        outlook.no_trade_reason.clone().unwrap_or_default(),
        outlook.d1_bias,
        outlook.h4_bias,
        strategist
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn vn_at(y: i32, m: u32, d: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        // The helper expects Vietnam local time expressed in a UTC-shaped value.
        Utc.with_ymd_and_hms(y, m, d, hour, minute, 0)
            .single()
            .expect("valid Vietnam local time")
    }

    #[test]
    fn daily_plan_is_due_at_or_after_13_vn_on_weekdays() {
        let monday = vn_at(2026, 11, 16, 13, 0);
        assert!(daily_plan_due(monday, None));
        assert!(!daily_plan_due(vn_at(2026, 11, 16, 12, 59), None));
        assert!(!daily_plan_due(monday, Some(monday.date_naive())));
    }

    #[test]
    fn daily_plan_is_not_due_on_weekends() {
        assert!(!daily_plan_due(vn_at(2026, 11, 14, 13, 0), None)); // Saturday
        assert!(!daily_plan_due(vn_at(2026, 11, 15, 13, 0), None)); // Sunday
    }
}
