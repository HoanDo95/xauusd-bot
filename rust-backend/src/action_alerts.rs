//! Read-only SETUP WATCHING alerts for the PRIMARY reference scenario.
//!
//! The worker watches the latest closed M15 candle and sends **one** Telegram
//! alert when price is close to the PRIMARY scenario's structured trigger but
//! the candle has NOT confirmed it. The message contains only the zone, trigger
//! and distance — never Entry/SL/TP/Lot. The confirmed full signal is owned by
//! the scanner. This worker is intentionally independent from the trade-signal
//! pipeline: it never inserts a signal, consumes the daily Sniper quota, calls
//! the AI review or places an order.
//!
//! Anti-spam rule: once an alert is sent, the setup is stored in
//! `bot_state.json` and the worker stays silent until the setup triggers (the
//! scanner owns the GOLD SIGNAL), reaches its structural stop, or expires after
//! 8 M15 bars. Alerts only run inside the 13:00–23:00 GMT+7 session.

use crate::api::{current_scenarios, persist_state, AppState};
use crate::market_view::MarketScore;
use crate::models::{Candle, Timeframe};
use crate::rsi::rsi_snapshot;
use crate::scenarios::{
    scenario_triggered, Scenario, ScenarioMarket, WatchedSetup, WATCH_SETUP_MAX_BARS,
};
use crate::storage::PersistentStore;
use crate::trigger_journal::{record, regime_label, TriggerEvent};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};

/// Worker polling cadence.
const ACTION_TRIGGER_INTERVAL_SECS: u64 = 30;
/// Maximum number of persisted zone-cooldown entries kept.
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

/// Zone key: scenario id + direction + trigger rounded to a zone bucket.
///
/// Using the exact trigger price caused spam because the scenario builder moved
/// the trigger by a few points on almost every M15 rebuild. A bucket (half of
/// H4 ATR, minimum 5 points) treats those as the same support/resistance zone.
pub fn watch_zone_key(scenario: &Scenario, trigger_price: f64, zone_size: f64) -> String {
    let zone_size = zone_size.max(1.0);
    let bucket = (trigger_price / zone_size).round() * zone_size;
    format!(
        "{}:{}:{:.2}",
        scenario.id,
        scenario.trigger_direction.trim(),
        bucket
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
            "👀 [Setup Watching] Worker cảnh báo vùng theo dõi đã khởi chạy (chu kỳ {} giây)...",
            ACTION_TRIGGER_INTERVAL_SECS
        );
        let mut interval = tokio::time::interval(Duration::from_secs(ACTION_TRIGGER_INTERVAL_SECS));
        let mut last_observed_candle: Option<i64> = None;

        loop {
            interval.tick().await;

            let config = state.config.read().await.clone();
            if !config.is_active {
                continue;
            }
            if PersistentStore::is_market_closed_weekend(Utc::now()) {
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

            // Never add a watching alert after a full signal was already
            // produced for this candle.
            let has_signal_for_candle = {
                let signals = state.signals.read().await;
                signals.iter().any(|signal| {
                    !signal.is_simulated && signal.candle_timestamp == candle.timestamp
                })
            };
            if has_signal_for_candle {
                continue;
            }

            let scenarios = current_scenarios(&state).await;
            let primary = scenarios
                .scenarios
                .iter()
                .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"));
            let in_session =
                PersistentStore::is_within_golden_session(now, config.session_filter_enabled);
            let volume_ratio = {
                let candles = state.candles.read().await;
                crate::market_strategist::volume_surge_ratio(&candles, 20)
            };
            let mut context = state.market_context.read().await.clone();
            if let Some(context) = context.as_mut() {
                if context.rsi_m15.is_none() {
                    let candles = state.candles.read().await;
                    context.rsi_m15 = Some(rsi_snapshot(
                        &candles,
                        config.rsi_period,
                        config.rsi_overbought,
                        config.rsi_oversold,
                        config.rsi_divergence_lookback,
                        config.rsi_pivot_k,
                    ));
                }
            }
            let score =
                crate::market_view::score_market(&scenarios, context.as_ref(), volume_ratio);

            // One raw scenario observation per M15 close, including outside the
            // session, so the journal can measure trigger frequency/evolution.
            if config.trigger_journal_enabled && last_observed_candle != Some(candle.timestamp) {
                if let Some(primary) = primary {
                    let event = observation_event(
                        primary,
                        scenarios.market.as_ref(),
                        &candle,
                        now,
                        in_session,
                        score.as_ref(),
                    );
                    record(&state.trigger_journal, true, &event);
                    last_observed_candle = Some(candle.timestamp);
                }
            }

            if !in_session {
                continue;
            }

            // Hard NO_TRADE gate: abnormal spread proxy blocks watching alerts
            // (the Market View itself stays available with the reason).
            if let Some(spread) = crate::api::spread_proxy_atr(&state, &candle).await {
                if spread > config.max_spread_proxy_atr {
                    continue;
                }
            }

            // One watching alert per setup lifecycle. While a setup is active,
            // stay silent: the scanner owns the confirmed GOLD SIGNAL.
            let active_setup = state.active_watch_setup.read().await.clone();
            if let Some(setup) = active_setup {
                if setup.is_triggered(&candle) {
                    continue;
                }
                if setup.is_expired(now, WATCH_SETUP_MAX_BARS)
                    || setup.is_invalidated(&candle, None)
                {
                    let reason = if setup.is_expired(now, WATCH_SETUP_MAX_BARS) {
                        "expired"
                    } else {
                        "invalidated"
                    };
                    let event = setup_closed_event(&setup, &candle, now, reason);
                    *state.active_watch_setup.write().await = None;
                    record(
                        &state.trigger_journal,
                        config.trigger_journal_enabled,
                        &event,
                    );
                    if let Err(err) = persist_state(&state).await {
                        warn!(
                            "⚠️ [Setup Watching] Không lưu được trạng thái clear setup: {}",
                            err
                        );
                    }
                } else {
                    continue;
                }
            }

            let Some(primary) = primary else {
                continue;
            };
            let Some(trigger_price) = primary.trigger_price.filter(|value| value.is_finite())
            else {
                continue;
            };

            if !primary.valid
                || primary.macro_alignment.eq_ignore_ascii_case("COUNTER")
                || primary.confidence < config.action_watch_min_confidence
            {
                continue;
            }

            // The confirmed full signal is owned by the scanner. This worker
            // only warns that price is approaching the trigger.
            if scenario_triggered(primary, &candle) {
                continue;
            }

            let Some(market) = scenarios.market.as_ref() else {
                continue;
            };
            let distance = (candle.close - trigger_price).abs();
            let watch_distance = (market.h4_atr.max(1.0) * config.action_watch_distance_atr)
                .min(config.action_watch_max_distance_points);
            if distance > watch_distance {
                continue;
            }

            let direction = primary.trigger_direction.trim().to_uppercase();
            let approaching = match direction.as_str() {
                "BELOW" => candle.close > trigger_price,
                "ABOVE" => candle.close < trigger_price,
                "TOUCH" => true,
                _ => false,
            };
            if !approaching {
                continue;
            }

            let zone_size = (market.h4_atr * 0.5).max(5.0);
            let key = watch_zone_key(primary, trigger_price, zone_size);
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

            let telegram = state.telegram.read().await.clone();
            match telegram
                .send_setup_watching_alert(primary, candle.close, distance)
                .await
            {
                Ok(msg_id) => {
                    info!(
                        "👀 [Setup Watching] {} {} · M15 close {:.2} · trigger {:.2} · distance {:.2} · msg_id={}",
                        primary.id, primary.trigger_direction, candle.close, trigger_price, distance, msg_id
                    );
                    let setup = WatchedSetup {
                        setup_id: format!("SETUP-{}-{}", now.timestamp_millis(), primary.id),
                        scenario: primary.clone(),
                        created_at: now,
                        created_candle_timestamp: candle.timestamp,
                        alerted_price: candle.close,
                    };
                    *state.active_watch_setup.write().await = Some(setup.clone());
                    let event = setup_created_event(
                        &setup,
                        Some(market),
                        &candle,
                        now,
                        distance,
                        watch_distance,
                        score.as_ref(),
                    );
                    record(
                        &state.trigger_journal,
                        config.trigger_journal_enabled,
                        &event,
                    );
                    {
                        let mut last_sent = state.action_trigger_last_sent.write().await;
                        last_sent.insert(key, now);
                        prune_last_sent(&mut last_sent, MAX_ACTION_TRIGGER_DEDUP);
                    }
                    if let Err(err) = persist_state(&state).await {
                        warn!(
                            "⚠️ [Setup Watching] Không lưu được trạng thái setup: {}",
                            err
                        );
                    }
                }
                Err(err) => {
                    warn!(
                        "⚠️ [Setup Watching] Không gửi được cảnh báo {}: {}",
                        primary.id, err
                    );
                }
            }
        }
    });
}

fn fill_scenario_fields(
    event: &mut TriggerEvent,
    scenario: &Scenario,
    market: Option<&ScenarioMarket>,
    candle: &Candle,
    in_session: bool,
) {
    event.scenario_id = Some(scenario.id.clone());
    event.scenario_name = Some(scenario.name.clone());
    event.direction = Some(scenario.trigger_direction.clone());
    event.bias = Some(scenario.bias.clone());
    event.trigger = scenario.trigger_price;
    event.entry = scenario.entry;
    event.stop_loss = scenario.stop_loss;
    event.take_profit_1 = scenario.take_profit_1;
    event.take_profit_2 = scenario.take_profit_2;
    event.risk_points = scenario
        .entry
        .zip(scenario.stop_loss)
        .map(|(entry, stop)| (entry - stop).abs());
    event.confidence = Some(scenario.confidence);
    event.valid = Some(scenario.valid);
    event.macro_alignment = Some(scenario.macro_alignment.clone());
    event.session = Some(if in_session { "IN" } else { "OUT" }.to_string());
    event.regime =
        market.map(|market| regime_label(&market.d1_bias, &market.h4_bias, &market.h1_bias));
    event.h4_atr = market.map(|market| market.h4_atr);
    event.distance = scenario
        .trigger_price
        .map(|trigger| (candle.close - trigger).abs());
}

fn observation_event(
    scenario: &Scenario,
    market: Option<&ScenarioMarket>,
    candle: &Candle,
    now: DateTime<Utc>,
    in_session: bool,
    score: Option<&MarketScore>,
) -> TriggerEvent {
    let mut event = TriggerEvent::new("scenario_observed");
    event.created_at = Some(now);
    event.candle_timestamp = Some(candle.timestamp);
    event.source = Some("scenario_observe".to_string());
    fill_scenario_fields(&mut event, scenario, market, candle, in_session);
    fill_score(&mut event, score);
    event
}

fn setup_created_event(
    setup: &WatchedSetup,
    market: Option<&ScenarioMarket>,
    candle: &Candle,
    now: DateTime<Utc>,
    distance: f64,
    watch_distance: f64,
    score: Option<&MarketScore>,
) -> TriggerEvent {
    let mut event = TriggerEvent::new("setup_created");
    event.created_at = Some(now);
    event.setup_id = Some(setup.effective_setup_id());
    event.candle_timestamp = Some(candle.timestamp);
    event.source = Some("watching_alert".to_string());
    fill_scenario_fields(&mut event, &setup.scenario, market, candle, true);
    event.distance = Some(distance);
    event.watch_distance = Some(watch_distance);
    fill_score(&mut event, score);
    event
}

fn fill_score(event: &mut TriggerEvent, score: Option<&MarketScore>) {
    if let Some(score) = score {
        event.score_total = Some(score.total);
        event.score_structure = Some(score.structure);
        event.score_trigger = Some(score.trigger);
        event.score_macro = Some(score.macro_score);
    }
}

fn setup_closed_event(
    setup: &WatchedSetup,
    candle: &Candle,
    now: DateTime<Utc>,
    reason: &str,
) -> TriggerEvent {
    let mut event = TriggerEvent::new("setup_closed");
    event.created_at = Some(now);
    event.setup_id = Some(setup.effective_setup_id());
    event.scenario_id = Some(setup.scenario.id.clone());
    event.candle_timestamp = Some(candle.timestamp);
    event.reason = Some(reason.to_string());
    let bars = (candle.timestamp - setup.created_candle_timestamp) / (15 * 60 * 1000);
    event.bars_open = Some(bars.max(0));
    event.source = Some("watching_alert".to_string());
    event
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

    fn market() -> ScenarioMarket {
        ScenarioMarket {
            h4_bias: "BULLISH".to_string(),
            h1_bias: "BULLISH".to_string(),
            d1_bias: "BULLISH".to_string(),
            h4_atr: 12.0,
            nearest_support: Some(2690.0),
            nearest_resistance: Some(2710.0),
            rsi_m15: Some(55.0),
            rsi_state: "Neutral".to_string(),
            macro_bias: "BULLISH".to_string(),
            macro_confidence: 80,
            daily_change_pct: Some(0.5),
            rsi_h1: None,
            rsi_h4: None,
            rsi_d1: None,
        }
    }

    #[test]
    fn observation_event_carries_scenario_and_session() {
        let scenario = scenario(true, Some(2701.0), "ABOVE");
        let event = observation_event(
            &scenario,
            Some(&market()),
            &candle(BASE_TS, 2705.0, 2695.0, 2700.0),
            fixed_now(),
            false,
            None,
        );
        assert_eq!(event.event, "scenario_observed");
        assert_eq!(event.scenario_id.as_deref(), Some("bullish_breakout"));
        assert_eq!(event.trigger, Some(2701.0));
        assert_eq!(event.session.as_deref(), Some("OUT"));
        assert_eq!(event.regime.as_deref(), Some("TREND_BULL"));
        assert_eq!(event.valid, Some(true));
    }

    #[test]
    fn setup_created_event_carries_frozen_levels_and_distance() {
        let setup = WatchedSetup {
            setup_id: "SETUP-1".to_string(),
            scenario: scenario(true, Some(2701.0), "ABOVE"),
            created_at: fixed_now(),
            created_candle_timestamp: BASE_TS,
            alerted_price: 2705.0,
        };
        let event = setup_created_event(
            &setup,
            Some(&market()),
            &candle(BASE_TS, 2705.0, 2695.0, 2705.0),
            fixed_now(),
            4.0,
            10.0,
            None,
        );
        assert_eq!(event.event, "setup_created");
        assert_eq!(event.setup_id.as_deref(), Some("SETUP-1"));
        assert_eq!(event.distance, Some(4.0));
        assert_eq!(event.watch_distance, Some(10.0));
        assert_eq!(event.source.as_deref(), Some("watching_alert"));
    }

    #[test]
    fn setup_closed_event_records_reason_and_bars() {
        let setup = WatchedSetup {
            setup_id: "SETUP-1".to_string(),
            scenario: scenario(true, Some(2701.0), "ABOVE"),
            created_at: fixed_now(),
            created_candle_timestamp: BASE_TS,
            alerted_price: 2705.0,
        };
        let event = setup_closed_event(
            &setup,
            &candle(BASE_TS + 2 * 15 * 60 * 1000, 2700.0, 2690.0, 2695.0),
            fixed_now(),
            "expired",
        );
        assert_eq!(event.event, "setup_closed");
        assert_eq!(event.reason.as_deref(), Some("expired"));
        assert_eq!(event.bars_open, Some(2));
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
    fn watch_zone_key_includes_id_direction_and_bucket() {
        let scenario = scenario(true, Some(2701.0), "ABOVE");

        assert_eq!(
            watch_zone_key(&scenario, 2701.0, 10.0),
            "bullish_breakout:ABOVE:2700.00"
        );
    }

    #[test]
    fn watch_zone_key_keeps_nearby_triggers_in_same_bucket() {
        let scenario = scenario(true, Some(2701.0), "ABOVE");

        assert_eq!(
            watch_zone_key(&scenario, 2701.0, 10.0),
            watch_zone_key(&scenario, 2702.0, 10.0)
        );
        assert_ne!(
            watch_zone_key(&scenario, 2701.0, 10.0),
            watch_zone_key(&scenario, 2710.0, 10.0)
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
    fn watch_zone_key_changes_for_a_different_direction() {
        let scenario = scenario(true, Some(2701.0), "ABOVE");
        let mut opposite = scenario.clone();
        opposite.trigger_direction = "BELOW".to_string();

        assert_ne!(
            watch_zone_key(&scenario, 2701.0, 10.0),
            watch_zone_key(&opposite, 2701.0, 10.0)
        );
    }
}
