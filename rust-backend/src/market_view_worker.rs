//! H1 MARKET VIEW Telegram worker.
//!
//! The dashboard/API computes the view 24/5. This worker only sends a Telegram
//! message on a new H1 close during the 13:00–23:00 session, and only when the
//! view changed materially and the cooldown elapsed. It never inserts a signal,
//! consumes quota or places an order.

use crate::api::{compute_market_outlook, AppState};
use crate::market_view::MarketOutlook;
use crate::storage::PersistentStore;
use chrono::{DateTime, Duration, Utc};
use std::time::Duration as StdDuration;
use tracing::{info, warn};

const MARKET_VIEW_INTERVAL_SECS: u64 = 60;

pub fn spawn_market_view_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "🧭 [Market View] Worker H1 đã khởi chạy (chu kỳ {} giây)...",
            MARKET_VIEW_INTERVAL_SECS
        );
        let mut interval = tokio::time::interval(StdDuration::from_secs(MARKET_VIEW_INTERVAL_SECS));
        let mut last_h1_timestamp: Option<i64> = None;
        let mut last_key: Option<String> = None;
        let mut last_sent: Option<DateTime<Utc>> = None;

        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.is_active {
                continue;
            }
            if !config.market_view_telegram_enabled && !config.market_strategist_enabled {
                continue;
            }
            let h1_timestamp = {
                let context = state.market_context.read().await;
                context
                    .as_ref()
                    .map(|context| context.h1.last_candle_timestamp)
                    .unwrap_or(0)
            };
            if h1_timestamp <= 0 || last_h1_timestamp == Some(h1_timestamp) {
                continue;
            }
            last_h1_timestamp = Some(h1_timestamp);

            let now = Utc::now();
            if !PersistentStore::is_within_golden_session(now, config.session_filter_enabled) {
                continue;
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
                continue;
            }
            let key = outlook_key(&outlook);
            let cooldown = Duration::minutes(config.market_view_cooldown_minutes.max(15));
            let cooldown_elapsed = match last_sent {
                None => true,
                Some(sent) => now - sent >= cooldown,
            };
            if last_key.as_deref() == Some(key.as_str()) || !cooldown_elapsed {
                continue;
            }

            let telegram = state.telegram.read().await.clone();
            match telegram.send_market_view(&outlook).await {
                Ok(msg_id) => {
                    info!(
                        "🧭 [Market View] H1 {} · state {} · msg_id={}",
                        h1_timestamp,
                        outlook.bot_state.as_str(),
                        msg_id
                    );
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
