//! Read-only Action Trigger alerts for the PRIMARY reference scenario.
//!
//! The worker watches the latest closed M15 candle and sends one Telegram alert
//! when the PRIMARY scenario's structured trigger is hit. It is intentionally
//! independent from the trade-signal pipeline: it never inserts a signal,
//! consumes the daily Sniper quota, persists trade state, calls the AI review or
//! places an order. Dedup/cooldown state is kept in memory only.

use crate::api::{current_scenarios, AppState};
use crate::models::{Candle, Timeframe};
use crate::scenarios::{scenario_triggered, Scenario};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};

/// Worker polling cadence.
const ACTION_TRIGGER_INTERVAL_SECS: u64 = 30;
/// Maximum number of in-memory cooldown entries kept.
const MAX_ACTION_TRIGGER_DEDUP: usize = 50;
/// Cooldowns below 5 minutes are treated as 5 minutes to avoid Telegram spam.
const MIN_ACTION_TRIGGER_COOLDOWN_MINUTES: i64 = 5;

/// Decide whether a trigger alert may be sent for the PRIMARY scenario.
///
/// This only checks the gates available without a candle; the caller must still
/// call [`scenario_triggered`] with the latest closed candle. A missing/invalid
/// trigger price, an invalid scenario or a disabled toggle never sends.
pub fn should_send_trigger_alert(
    enabled: bool,
    valid: bool,
    trigger_price: Option<f64>,
    last_sent: Option<DateTime<Utc>>,
    cooldown_minutes: i64,
    now: DateTime<Utc>,
) -> bool {
    if !enabled || !valid {
        return false;
    }
    if !trigger_price.is_some_and(|value| value.is_finite()) {
        return false;
    }

    match last_sent {
        None => true,
        Some(last_sent) => {
            (now - last_sent).num_minutes()
                >= cooldown_minutes.max(MIN_ACTION_TRIGGER_COOLDOWN_MINUTES)
        }
    }
}

/// Dedup key: scenario id + direction + trigger price at 2 decimals.
pub fn trigger_dedup_key(scenario: &Scenario, trigger_price: f64) -> String {
    format!(
        "{}:{}:{:.2}",
        scenario.id,
        scenario.trigger_direction.trim(),
        trigger_price
    )
}

/// A candle is usable only after it has closed and before it becomes stale.
///
/// `max_age_secs` mirrors `BotConfig::max_candle_age_secs`; a candle whose close
/// time is in the future (clock skew or an open candle) is rejected.
pub fn candle_is_closed_and_fresh(candle: &Candle, now_ms: i64, max_age_secs: i64) -> bool {
    let close_ms = candle
        .timestamp
        .saturating_add(timeframe_millis(candle.timeframe));
    let age_ms = now_ms.saturating_sub(close_ms);
    age_ms >= 0 && age_ms <= max_age_secs.max(0).saturating_mul(1000)
}

/// Keep only the newest `max` cooldown entries.
fn prune_last_sent(last_sent: &mut HashMap<String, DateTime<Utc>>, max: usize) {
    if last_sent.len() <= max {
        return;
    }

    let mut entries: Vec<(String, DateTime<Utc>)> = last_sent
        .iter()
        .map(|(key, at)| (key.clone(), *at))
        .collect();
    entries.sort_by_key(|(_, at)| *at);

    let remove_count = last_sent.len() - max;
    for (key, _) in entries.into_iter().take(remove_count) {
        last_sent.remove(&key);
    }
}

fn timeframe_millis(timeframe: Timeframe) -> i64 {
    match timeframe {
        Timeframe::M15 => 15 * 60 * 1000,
        Timeframe::H1 => 60 * 60 * 1000,
        Timeframe::H4 => 4 * 60 * 60 * 1000,
    }
}

/// Spawn the read-only action-trigger worker (30-second cadence).
///
/// Every tick reuses the shared read-only scenario builder, so trigger levels
/// stay consistent with `/api/scenarios` and the Action View. Only the PRIMARY
/// scenario is eligible, and only when it is `valid` with a trigger price.
pub fn spawn_action_trigger_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "🚨 [Action Trigger] Worker cảnh báo trigger kịch bản đã khởi chạy (chu kỳ {} giây)...",
            ACTION_TRIGGER_INTERVAL_SECS
        );
        let mut interval = tokio::time::interval(Duration::from_secs(ACTION_TRIGGER_INTERVAL_SECS));

        loop {
            interval.tick().await;

            let config = state.config.read().await.clone();
            if !config.action_trigger_alert_enabled || !config.is_active {
                continue;
            }

            // Never alert on synthetic/stale fallback candles.
            let active_data_source = state.active_data_source.read().await.clone();
            if active_data_source.eq_ignore_ascii_case("FALLBACK_SEED")
                || active_data_source.eq_ignore_ascii_case("SPOT_BASIS_UNAVAILABLE")
            {
                continue;
            }

            let candle = {
                let candles = state.candles.read().await;
                candles.last().cloned()
            };
            let Some(candle) = candle else {
                continue;
            };

            let now = Utc::now();
            if !candle_is_closed_and_fresh(
                &candle,
                now.timestamp_millis(),
                config.max_candle_age_secs,
            ) {
                continue;
            }

            let scenarios = current_scenarios(&state).await;
            let Some(primary) = scenarios
                .scenarios
                .iter()
                .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"))
            else {
                continue;
            };

            let Some(trigger_price) = primary.trigger_price.filter(|value| value.is_finite())
            else {
                continue;
            };

            let key = trigger_dedup_key(primary, trigger_price);
            let last_sent = state
                .action_trigger_last_sent
                .read()
                .await
                .get(&key)
                .copied();

            if !should_send_trigger_alert(
                config.action_trigger_alert_enabled,
                primary.valid,
                primary.trigger_price,
                last_sent,
                config.action_trigger_cooldown_minutes,
                now,
            ) {
                continue;
            }

            if !scenario_triggered(primary, &candle) {
                continue;
            }

            let telegram = state.telegram.read().await.clone();
            match telegram
                .send_action_trigger_alert(primary, candle.close)
                .await
            {
                Ok(msg_id) => {
                    info!(
                        "🚨 [Action Trigger] {} {} · M15 close {:.2} · trigger {:.2} · msg_id={}",
                        primary.id, primary.trigger_direction, candle.close, trigger_price, msg_id
                    );
                    let mut last_sent = state.action_trigger_last_sent.write().await;
                    last_sent.insert(key, now);
                    prune_last_sent(&mut last_sent, MAX_ACTION_TRIGGER_DEDUP);
                }
                Err(err) => {
                    warn!(
                        "⚠️ [Action Trigger] Không gửi được cảnh báo {}: {}",
                        primary.id, err
                    );
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const BASE_TS: i64 = 1_700_000_000_000;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 12, 12, 0, 0)
            .single()
            .expect("valid datetime")
    }

    fn candle(timestamp: i64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp,
            open: close,
            high,
            low,
            close,
            volume: 1000.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    fn scenario(valid: bool, trigger_price: Option<f64>, direction: &str) -> Scenario {
        Scenario {
            id: "bullish_breakout".to_string(),
            name: "Bullish breakout".to_string(),
            bias: "BULLISH".to_string(),
            trigger: "Đóng M15 trên 2701.00".to_string(),
            condition: "Phá vỡ kháng cự gần nhất".to_string(),
            entry: Some(2701.0),
            stop_loss: Some(2691.0),
            take_profit_1: Some(2721.0),
            take_profit_2: Some(2731.0),
            risk_reward: Some(2.0),
            trigger_price,
            trigger_direction: direction.to_string(),
            trigger_timeframe: "M15".to_string(),
            invalidation: "Đóng M15 dưới 2690.00".to_string(),
            confidence: 80,
            macro_alignment: "ALIGNED".to_string(),
            valid,
            role: "PRIMARY".to_string(),
            priority: 300,
            notes: Vec::new(),
        }
    }

    #[test]
    fn should_send_requires_enabled_valid_and_a_trigger_price() {
        let now = fixed_now();

        assert!(!should_send_trigger_alert(
            false,
            true,
            Some(2701.0),
            None,
            30,
            now
        ));
        assert!(!should_send_trigger_alert(
            true,
            false,
            Some(2701.0),
            None,
            30,
            now
        ));
        assert!(!should_send_trigger_alert(true, true, None, None, 30, now));
        assert!(!should_send_trigger_alert(
            true,
            true,
            Some(f64::NAN),
            None,
            30,
            now
        ));
        assert!(should_send_trigger_alert(
            true,
            true,
            Some(2701.0),
            None,
            30,
            now
        ));
    }

    #[test]
    fn should_send_enforces_cooldown_boundary() {
        let now = fixed_now();
        let last = now - chrono::Duration::minutes(29);
        assert!(!should_send_trigger_alert(
            true,
            true,
            Some(2701.0),
            Some(last),
            30,
            now
        ));

        let last = now - chrono::Duration::minutes(30);
        assert!(should_send_trigger_alert(
            true,
            true,
            Some(2701.0),
            Some(last),
            30,
            now
        ));

        let last = now - chrono::Duration::minutes(31);
        assert!(should_send_trigger_alert(
            true,
            true,
            Some(2701.0),
            Some(last),
            30,
            now
        ));
    }

    #[test]
    fn should_send_clamps_cooldown_to_five_minutes() {
        let now = fixed_now();
        let last = now - chrono::Duration::minutes(4);
        assert!(!should_send_trigger_alert(
            true,
            true,
            Some(2701.0),
            Some(last),
            0,
            now
        ));

        let last = now - chrono::Duration::minutes(5);
        assert!(should_send_trigger_alert(
            true,
            true,
            Some(2701.0),
            Some(last),
            0,
            now
        ));
    }

    #[test]
    fn should_send_blocks_clock_skew() {
        let now = fixed_now();
        let last = now + chrono::Duration::minutes(5);
        assert!(!should_send_trigger_alert(
            true,
            true,
            Some(2701.0),
            Some(last),
            30,
            now
        ));
    }

    #[test]
    fn trigger_dedup_key_includes_id_direction_and_price() {
        let scenario = scenario(true, Some(2701.0), "ABOVE");

        assert_eq!(
            trigger_dedup_key(&scenario, 2701.0),
            "bullish_breakout:ABOVE:2701.00"
        );
    }

    #[test]
    fn candle_must_be_closed_and_fresh() {
        let candle = candle(BASE_TS, 2705.0, 2695.0, 2701.0);
        let close_ms = BASE_TS + 15 * 60 * 1000;

        assert!(candle_is_closed_and_fresh(&candle, close_ms, 120));
        assert!(candle_is_closed_and_fresh(&candle, close_ms + 120_000, 120));
        assert!(!candle_is_closed_and_fresh(
            &candle,
            close_ms + 120_001,
            120
        ));
        assert!(!candle_is_closed_and_fresh(&candle, close_ms - 1, 120));
    }

    #[test]
    fn prune_last_sent_keeps_the_newest_entries() {
        let now = fixed_now();
        let mut last_sent = HashMap::new();
        for index in 0..55 {
            last_sent.insert(
                format!("scenario:{index}"),
                now - chrono::Duration::minutes(55 - index),
            );
        }

        prune_last_sent(&mut last_sent, 50);

        assert_eq!(last_sent.len(), 50);
        for index in 0..5 {
            assert!(!last_sent.contains_key(&format!("scenario:{index}")));
        }
        assert!(last_sent.contains_key("scenario:54"));
    }

    #[test]
    fn dedup_key_changes_when_the_trigger_level_moves() {
        let scenario = scenario(true, Some(2701.0), "ABOVE");

        assert_ne!(
            trigger_dedup_key(&scenario, 2701.0),
            trigger_dedup_key(&scenario, 2702.0)
        );
    }
}
