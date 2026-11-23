//! Realtime setup-alert engine.
//!
//! Watches every enabled, non-reference scenario on closed M15 bars and emits
//! three view-only events:
//! - `PRE_ALERT`: price is inside/near the scenario zone, trigger not confirmed.
//! - `TRIGGERED`: the M15 close confirmed the scenario trigger.
//! - `INVALIDATED`: the M15 close violated the scenario's structural level.
//!
//! Levels are built from the **previous** closed M15 bar, then evaluated against
//! the newly closed bar. This mirrors the scanner's no-lookahead approach and
//! prevents a breakout trigger from moving above price right after the break.
//!
//! The engine never inserts a signal, consumes quota, calls AI or places an
//! order. Every event is deduped by `scenario_id:transition:bar_timestamp` and
//! persisted in `bot_state.json`, so restarts cannot replay it.

use crate::api::{persist_state, AppState};
use crate::models::{BotConfig, Candle, Timeframe};
use crate::scenarios::{
    build_scenarios, is_reference_only_scenario, Scenario, ScenarioMarket, ScenarioResponse,
};
use crate::setup_state::{
    event_key, is_approaching, is_terminal, max_watch_bars, pre_alert_key, SetupAlertStore,
    SetupRecord, STATUS_PRE_ALERT, STATUS_TRIGGERED, STATUS_WATCHING,
};
use crate::storage::PersistentStore;
use crate::telegram::TelegramNotifier;
use crate::trigger_journal::{record, regime_label, TriggerEvent};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tracing::{info, warn};

/// Worker polling cadence. State transitions are still gated to a newly closed
/// M15 candle; the tick only decides when to check.
const ACTION_TRIGGER_INTERVAL_SECS: u64 = 30;
/// Maximum number of persisted zone-cooldown entries kept.
const MAX_ACTION_TRIGGER_DEDUP: usize = 50;
/// Cooldowns below 5 minutes are treated as 5 minutes to avoid Telegram spam.
const MIN_ACTION_TRIGGER_COOLDOWN_MINUTES: i64 = 5;

/// Backward-compatible gate used by older tests/tools. PRE_ALERT no longer uses
/// the old single-setup lifecycle, but the pure predicate remains useful.
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
pub fn watch_zone_key(scenario: &Scenario, trigger_price: f64, zone_size: f64) -> String {
    pre_alert_key(scenario, trigger_price, zone_size)
}

/// A candle is usable only after it has closed and before it becomes stale.
pub fn candle_is_closed_and_fresh(candle: &Candle, now_ms: i64, max_age_secs: i64) -> bool {
    let close_ms = candle
        .timestamp
        .saturating_add(timeframe_millis(candle.timeframe));
    let age_ms = now_ms.saturating_sub(close_ms);
    age_ms >= 0 && age_ms <= max_age_secs.max(0).saturating_mul(1000)
}

fn timeframe_millis(timeframe: Timeframe) -> i64 {
    match timeframe {
        Timeframe::M15 => 15 * 60 * 1000,
        Timeframe::H1 => 60 * 60 * 1000,
        Timeframe::H4 => 4 * 60 * 60 * 1000,
    }
}

fn prune_last_sent(last_sent: &mut HashMap<String, DateTime<Utc>>, max: usize) {
    if last_sent.len() <= max {
        return;
    }
    let mut entries: Vec<(String, DateTime<Utc>)> = last_sent
        .iter()
        .map(|(key, at)| (key.clone(), *at))
        .collect();
    entries.sort_by_key(|(_, at)| *at);
    for (key, _) in entries.into_iter().take(last_sent.len() - max) {
        last_sent.remove(&key);
    }
}

fn parse_extra_chat_ids(config: &BotConfig) -> Vec<String> {
    config
        .telegram_alert_chat_ids
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn scenario_enabled(scenario: &Scenario, config: &BotConfig) -> bool {
    if is_reference_only_scenario(&scenario.id) {
        return false;
    }
    if scenario.id.eq_ignore_ascii_case("bearish_rejection") && !config.bearish_rejection_enabled {
        return false;
    }
    true
}

/// The zone-view engine can alert on a scenario whose strict structural
/// `valid` flag is false (for example SL wider than the 45-point cap or TP1
/// below the 2R strict gate) as long as it still has a trigger and full levels.
/// The alert carries `STRUCTURE_INVALID` and remains a zone reference, not a
/// qualified signal.
fn scenario_alertable(scenario: &Scenario, config: &BotConfig) -> bool {
    scenario_enabled(scenario, config)
        && scenario
            .trigger_price
            .is_some_and(|value| value.is_finite() && value > 0.0)
        && !scenario
            .trigger_direction
            .trim()
            .eq_ignore_ascii_case("NONE")
        && scenario.entry.is_some()
        && scenario.stop_loss.is_some()
        && scenario.take_profit_1.is_some()
        && scenario.take_profit_2.is_some()
}

fn h4_atr(market: Option<&ScenarioMarket>) -> f64 {
    market
        .map(|market| market.h4_atr)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(10.0)
}

fn watch_distance(config: &BotConfig, market: Option<&ScenarioMarket>) -> f64 {
    (h4_atr(market) * config.action_watch_distance_atr)
        .min(config.action_watch_max_distance_points)
        .max(1.0)
}

fn zone_bounds(scenario: &Scenario, market: Option<&ScenarioMarket>) -> (f64, f64) {
    let anchor = scenario
        .entry
        .or(scenario.trigger_price)
        .unwrap_or_default();
    let buffer = (h4_atr(market) * 0.10).max(1.5);
    (round2(anchor - buffer), round2(anchor + buffer))
}

fn retest_bounds(scenario: &Scenario, market: Option<&ScenarioMarket>) -> (f64, f64) {
    let Some(trigger) = scenario
        .trigger_price
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return zone_bounds(scenario, market);
    };
    let buffer = (h4_atr(market) * 0.25).max(2.0);
    match scenario.trigger_direction.trim().to_uppercase().as_str() {
        "ABOVE" => (round2(trigger - buffer), round2(trigger + buffer * 0.25)),
        "BELOW" => (round2(trigger - buffer * 0.25), round2(trigger + buffer)),
        "TOUCH" => zone_bounds(scenario, market),
        _ => (round2(trigger - buffer), round2(trigger + buffer)),
    }
}

fn confirmation_text(scenario: &Scenario) -> String {
    let side = scenario.bias.trim().to_uppercase();
    match (
        scenario.trigger_price,
        scenario.trigger_direction.trim().to_uppercase().as_str(),
    ) {
        (Some(trigger), "ABOVE") => {
            format!("M15 close giữ trên {trigger:.2} sau retest")
        }
        (Some(trigger), "BELOW") => {
            format!("M15 close giữ dưới {trigger:.2} sau retest")
        }
        (Some(trigger), "TOUCH") => {
            let comparison = if side == "BEARISH" {
                "dưới"
            } else {
                "trên"
            };
            format!("M15 close {comparison} {trigger:.2} sau touch")
        }
        _ => "Chờ nến M15 đóng xác nhận trigger.".to_string(),
    }
}

/// Flags shown on every setup alert. They are context, never a mute switch.
fn setup_flags(
    scenario: &Scenario,
    market: Option<&ScenarioMarket>,
    in_session: bool,
) -> Vec<String> {
    let mut flags = Vec::new();
    let bias = scenario.bias.trim().to_uppercase();
    let macro_bias = market
        .map(|market| market.macro_bias.trim().to_uppercase())
        .unwrap_or_default();
    let counter_macro = scenario.macro_alignment.eq_ignore_ascii_case("COUNTER")
        || (macro_bias != "NEUTRAL"
            && macro_bias != "UNKNOWN"
            && !macro_bias.is_empty()
            && bias != macro_bias);
    if counter_macro {
        flags.push("COUNTER_TREND".to_string());
    }

    let rsi_m15 = market.and_then(|market| market.rsi_m15);
    let daily_change = market.and_then(|market| market.daily_change_pct);
    let high_risk = match bias.as_str() {
        "BULLISH" => {
            rsi_m15.is_some_and(|value| value >= 70.0)
                || daily_change.is_some_and(|value| value >= 2.0)
        }
        "BEARISH" => {
            rsi_m15.is_some_and(|value| value <= 30.0)
                || daily_change.is_some_and(|value| value <= -2.0)
        }
        _ => false,
    };
    if high_risk {
        flags.push("HIGH_RISK".to_string());
    }
    if !in_session {
        flags.push("OUT_SESSION".to_string());
    }
    if !scenario.valid {
        flags.push("STRUCTURE_INVALID".to_string());
    }
    if is_reference_only_scenario(&scenario.id) {
        flags.push("REFERENCE_ONLY".to_string());
    }
    flags
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Short market context shown under PRE_ALERT/TRIGGERED: RSI, daily change,
/// H4 ATR and macro confidence. Purely informational.
fn market_context_line(market: Option<&ScenarioMarket>) -> String {
    let Some(market) = market else {
        return "không có context".to_string();
    };
    let mut parts = Vec::new();
    if let Some(rsi) = market.rsi_m15.filter(|value| value.is_finite()) {
        parts.push(format!("RSI M15 {rsi:.1}"));
    }
    if let Some(change) = market.daily_change_pct.filter(|value| value.is_finite()) {
        parts.push(format!("daily {change:+.2}%"));
    }
    if market.h4_atr.is_finite() && market.h4_atr > 0.0 {
        parts.push(format!("H4 ATR {:.2}", market.h4_atr));
    }
    parts.push(format!(
        "macro {} {}%",
        market.macro_bias.trim(),
        market.macro_confidence
    ));
    parts.join(" · ")
}

fn pre_alert_cooldown_ok(
    store: &SetupAlertStore,
    key: &str,
    now: DateTime<Utc>,
    config: &BotConfig,
) -> bool {
    match store.pre_alert_last_sent(key) {
        None => true,
        Some(last) => {
            (now - last).num_minutes()
                >= config
                    .action_trigger_cooldown_minutes
                    .max(MIN_ACTION_TRIGGER_COOLDOWN_MINUTES)
        }
    }
}

fn setup_event(
    event_name: &str,
    setup: &SetupRecord,
    market: Option<&ScenarioMarket>,
    candle: &Candle,
    now: DateTime<Utc>,
    in_session: bool,
    reason: Option<&str>,
) -> TriggerEvent {
    let mut event = TriggerEvent::new(event_name);
    event.created_at = Some(now);
    event.setup_id = Some(setup.setup_id.clone());
    event.scenario_id = Some(setup.scenario.id.clone());
    event.scenario_name = Some(setup.scenario.name.clone());
    event.direction = Some(setup.scenario.trigger_direction.clone());
    event.bias = Some(setup.scenario.bias.clone());
    event.trigger = setup.scenario.trigger_price;
    event.entry = setup.scenario.entry;
    event.stop_loss = setup.scenario.stop_loss;
    event.take_profit_1 = setup.scenario.take_profit_1;
    event.take_profit_2 = setup.scenario.take_profit_2;
    event.risk_points = setup
        .scenario
        .entry
        .zip(setup.scenario.stop_loss)
        .map(|(entry, stop)| (entry - stop).abs());
    event.confidence = Some(setup.scenario.confidence);
    event.valid = Some(setup.scenario.valid);
    event.macro_alignment = Some(setup.scenario.macro_alignment.clone());
    event.candle_timestamp = Some(candle.timestamp);
    event.session = Some(if in_session { "IN" } else { "OUT" }.to_string());
    event.regime =
        market.map(|market| regime_label(&market.d1_bias, &market.h4_bias, &market.h1_bias));
    event.h4_atr = market.map(|market| market.h4_atr);
    event.bars_open = Some(setup.bars_open(candle));
    event.distance = setup
        .scenario
        .trigger_price
        .map(|trigger| (candle.close - trigger).abs());
    event.reason = reason.map(str::to_string);
    event.source = Some("setup_engine".to_string());
    event
}

fn observation_event(
    scenario: &Scenario,
    market: Option<&ScenarioMarket>,
    candle: &Candle,
    now: DateTime<Utc>,
    in_session: bool,
) -> TriggerEvent {
    let setup = SetupRecord::new(
        format!("OBS-{}-{}", candle.timestamp, scenario.id),
        scenario.clone(),
        now,
        candle.timestamp,
    );
    let mut event = setup_event(
        "scenario_observed",
        &setup,
        market,
        candle,
        now,
        in_session,
        None,
    );
    event.source = Some("scenario_observe".to_string());
    event
}

async fn live_scenarios(
    state: &AppState,
    config: &BotConfig,
    now: DateTime<Utc>,
) -> Option<ScenarioResponse> {
    let candles = state.candles.read().await.clone();
    // Build from the previous closed bar, then evaluate the new bar. This is the
    // same no-lookahead rule used by the scanner's confirmed-signal path.
    let setup_candles = candles
        .get(..candles.len().saturating_sub(1))
        .filter(|candles| candles.len() >= config.lookback_period.saturating_add(5))?;
    let setup_price = setup_candles.last().map(|candle| candle.close);
    let context = state.market_context.read().await.clone();
    let macro_flow = state.macro_flow.read().await.clone();
    let source = state.active_data_source.read().await.clone();
    let volume_source = state.active_volume_source.read().await.clone();
    Some(build_scenarios(
        setup_candles,
        config,
        setup_price,
        context.as_ref(),
        macro_flow.as_ref(),
        &source,
        &volume_source,
        now.timestamp_millis(),
    ))
}

async fn send_setup_message(
    telegram: &TelegramNotifier,
    text: &str,
    extra_chat_ids: &[String],
    event_label: &str,
    setup_id: &str,
) -> bool {
    match telegram.send_setup_alert_text(text, extra_chat_ids).await {
        Ok(sent) => {
            if sent.is_empty() {
                info!(
                    "⚠️ [Setup Engine] {} {} không có chat hợp lệ (simulator/no chat).",
                    event_label, setup_id
                );
            } else {
                info!(
                    "📨 [Setup Engine] {} {} gửi {} chat.",
                    event_label,
                    setup_id,
                    sent.len()
                );
            }
            true
        }
        Err(err) => {
            warn!(
                "⚠️ [Setup Engine] Không gửi được {} {}: {}",
                event_label, setup_id, err
            );
            false
        }
    }
}

/// Evaluate one tick. Returns after the new bar has been fully handled.
async fn evaluate_once(state: &AppState) -> Result<(), String> {
    let config = state.config.read().await.clone();
    if !config.is_active || !config.action_trigger_alert_enabled {
        return Ok(());
    }
    let now = Utc::now();
    if PersistentStore::is_market_closed_weekend(now) {
        return Ok(());
    }

    let source = state.active_data_source.read().await.clone();
    if source.eq_ignore_ascii_case("FALLBACK_SEED")
        || source.eq_ignore_ascii_case("SPOT_BASIS_UNAVAILABLE")
    {
        return Ok(());
    }

    let (candle, previous) = {
        let candles = state.candles.read().await;
        let candle = candles.last().cloned();
        let previous = candles.iter().rev().nth(1).cloned();
        (candle, previous)
    };
    let Some(candle) = candle else {
        return Ok(());
    };
    if !candle_is_closed_and_fresh(&candle, now.timestamp_millis(), config.max_candle_age_secs) {
        return Ok(());
    }

    // Cheap guard before the scenario rebuild: one evaluation per closed bar.
    let mut store = state.setup_alerts.read().await.clone();
    if store.last_bar_timestamp == Some(candle.timestamp) {
        return Ok(());
    }

    // A confirmed scanner GOLD SIGNAL already owns this candle. Do not send a
    // second setup TRIGGERED alert for the same bar.
    let has_full_signal_for_candle = {
        let signals = state.signals.read().await;
        signals
            .iter()
            .any(|signal| !signal.is_simulated && signal.candle_timestamp == candle.timestamp)
    };
    if has_full_signal_for_candle {
        store.last_bar_timestamp = Some(candle.timestamp);
        *state.setup_alerts.write().await = store;
        return persist_state(state).await;
    }

    let Some(scenarios) = live_scenarios(state, &config, now).await else {
        return Ok(());
    };
    if scenarios.status != "ok" || scenarios.scenarios.is_empty() {
        return Ok(());
    }
    if config.trigger_journal_enabled {
        let summary = scenarios
            .scenarios
            .iter()
            .map(|scenario| {
                format!(
                    "{}={} ({})",
                    scenario.id,
                    if scenario.valid { "valid" } else { "invalid" },
                    scenario.role
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        info!("🔎 [Setup Engine] M15 {} · {}", candle.timestamp, summary);
    }
    let market = scenarios.market.as_ref();
    // Judge the session at the bar close, not at the 30-second polling tick, so
    // a bar closing at 20:00 is OUT and one closing at 07:00 is IN.
    let candle_close = DateTime::<Utc>::from_timestamp_millis(
        candle
            .timestamp
            .saturating_add(timeframe_millis(candle.timeframe)),
    )
    .unwrap_or(now);
    let in_session =
        PersistentStore::is_within_split_session(candle_close, config.session_filter_enabled);

    if let Some(spread) = crate::api::spread_proxy_atr(state, &candle).await {
        if spread > config.max_spread_proxy_atr {
            return Ok(());
        }
    }

    // Preserve the legacy one-observation-per-M15 telemetry even though the
    // watcher now tracks every enabled scenario.
    if config.trigger_journal_enabled
        && !state
            .trigger_journal
            .has_event_for_candle("scenario_observed", candle.timestamp)
    {
        if let Some(primary) = scenarios
            .scenarios
            .iter()
            .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"))
        {
            let event = observation_event(primary, market, &candle, now, in_session);
            record_journal(state, &config, &event);
        }
    }

    let watch_distance = watch_distance(&config, market);
    let zone_size = (h4_atr(market) * 0.5).max(5.0);
    let context_line = market_context_line(market);
    let extra_chat_ids = parse_extra_chat_ids(&config);
    let telegram = state.telegram.read().await.clone();
    let mut all_ok = true;
    let mut closed_this_tick: HashSet<String> = HashSet::new();

    // 1) Existing setups: invalidate, trigger, pre-alert, expire.
    let tracked_ids: Vec<String> = store.setups.keys().cloned().collect();
    for id in tracked_ids {
        let Some(record) = store.setups.get(&id).cloned() else {
            continue;
        };
        if is_terminal(&record.status) {
            // Terminal records must not block a new lifecycle for the same
            // scenario id. The journal already has the invalidation event.
            store.setups.remove(&id);
            continue;
        }
        // A scenario disabled after the setup was armed must not fire a normal
        // TRIGGERED alert. Drop it silently; the report still shows it as
        // reference/disabled context.
        if !scenario_enabled(&record.scenario, &config) {
            store.setups.remove(&id);
            continue;
        }

        if record.is_expired(now, max_watch_bars())
            || record.bars_since_anchor(&candle) >= max_watch_bars()
        {
            let reason = if record.status == STATUS_TRIGGERED {
                "triggered_expired"
            } else {
                "expired"
            };
            let event = setup_event(
                "setup_expired",
                &record,
                market,
                &candle,
                now,
                in_session,
                Some(reason),
            );
            record_journal(state, &config, &event);
            store.setups.remove(&id);
            closed_this_tick.insert(id);
            continue;
        }

        if record.is_invalidated(&candle) {
            // Never alert INVALIDATED for a setup the user never saw: silently
            // tracked scenarios are context, not a watched plan.
            let was_visible = record.pre_alerted
                || record.status == STATUS_PRE_ALERT
                || record.status == STATUS_TRIGGERED;
            if !was_visible {
                store.setups.remove(&id);
                closed_this_tick.insert(id);
                continue;
            }
            let key = event_key(&id, "invalidated", candle.timestamp);
            let level = record
                .scenario
                .invalidation_price()
                .or(record.scenario.stop_loss)
                .unwrap_or(candle.close);
            let reason = format!(
                "M15 close {:.2} vi phạm invalidation {:.2}",
                candle.close, level
            );

            if !store.has_sent(&key) {
                let flags = setup_flags(&record.scenario, market, in_session);
                let text = TelegramNotifier::format_invalidated_message(
                    &record.scenario,
                    candle.close,
                    in_session,
                    &flags,
                    &reason,
                );
                if send_setup_message(
                    &telegram,
                    &text,
                    &extra_chat_ids,
                    "INVALIDATED",
                    &record.setup_id,
                )
                .await
                {
                    let mut updated = record.clone();
                    updated.mark_invalidated(now, candle.timestamp);
                    store.setups.remove(&id);
                    closed_this_tick.insert(id.clone());
                    store.mark_sent(key, now);
                    let event = setup_event(
                        "setup_invalidated",
                        &updated,
                        market,
                        &candle,
                        now,
                        in_session,
                        Some(&reason),
                    );
                    record_journal(state, &config, &event);
                } else {
                    all_ok = false;
                }
            } else {
                // Already alerted on this bar; close the lifecycle so a future
                // valid setup can be armed again.
                store.setups.remove(&id);
                closed_this_tick.insert(id);
            }
            continue;
        }

        if record.status != STATUS_TRIGGERED && record.trigger_crossed(previous.as_ref(), &candle) {
            let key = event_key(&id, "triggered", candle.timestamp);
            if !store.has_sent(&key) {
                let (zone_low, zone_high) = zone_bounds(&record.scenario, market);
                let (retest_low, retest_high) = retest_bounds(&record.scenario, market);
                let flags = setup_flags(&record.scenario, market, in_session);
                let confirmation = confirmation_text(&record.scenario);
                let text = TelegramNotifier::format_triggered_message(
                    &record.scenario,
                    candle.close,
                    in_session,
                    &flags,
                    zone_low,
                    zone_high,
                    retest_low,
                    retest_high,
                    &confirmation,
                    &context_line,
                );
                if send_setup_message(
                    &telegram,
                    &text,
                    &extra_chat_ids,
                    "TRIGGERED",
                    &record.setup_id,
                )
                .await
                {
                    let mut updated = record.clone();
                    updated.mark_triggered(now, candle.timestamp, candle.close);
                    store.setups.insert(id.clone(), updated.clone());
                    store.mark_sent(key, now);
                    let event = setup_event(
                        "setup_triggered",
                        &updated,
                        market,
                        &candle,
                        now,
                        in_session,
                        None,
                    );
                    record_journal(state, &config, &event);
                } else {
                    all_ok = false;
                }
            } else {
                let mut updated = record.clone();
                updated.mark_triggered(now, candle.timestamp, candle.close);
                store.setups.insert(id.clone(), updated);
            }
            continue;
        }

        if record.status == STATUS_WATCHING
            && !record.pre_alerted
            && scenario_enabled(&record.scenario, &config)
            && record.scenario.confidence >= config.action_watch_min_confidence
        {
            let Some(trigger_price) = record
                .scenario
                .trigger_price
                .filter(|value| value.is_finite())
            else {
                continue;
            };
            let distance = (candle.close - trigger_price).abs();
            if !is_approaching(&record.scenario, candle.close, watch_distance) {
                continue;
            }
            let pre_key = pre_alert_key(&record.scenario, trigger_price, zone_size);
            if !pre_alert_cooldown_ok(&store, &pre_key, now, &config) {
                continue;
            }
            let key = event_key(&id, "pre_alert", candle.timestamp);
            if store.has_sent(&key) {
                continue;
            }

            let (zone_low, zone_high) = zone_bounds(&record.scenario, market);
            let distance_atr = market.and_then(|market| {
                let atr = market.h4_atr;
                (atr.is_finite() && atr > 0.0).then(|| distance / atr)
            });
            let flags = setup_flags(&record.scenario, market, in_session);
            let confirmation = confirmation_text(&record.scenario);
            let text = TelegramNotifier::format_pre_alert_message(
                &record.scenario,
                candle.close,
                distance,
                watch_distance,
                distance_atr,
                in_session,
                &flags,
                zone_low,
                zone_high,
                &confirmation,
                &context_line,
            );
            if send_setup_message(
                &telegram,
                &text,
                &extra_chat_ids,
                "PRE_ALERT",
                &record.setup_id,
            )
            .await
            {
                let mut updated = record.clone();
                updated.mark_pre_alert(now, candle.timestamp);
                store.setups.insert(id.clone(), updated.clone());
                store.mark_sent(key, now);
                store.mark_pre_alert_sent(pre_key, now);
                let event = setup_event(
                    "setup_pre_alert",
                    &updated,
                    market,
                    &candle,
                    now,
                    in_session,
                    None,
                );
                record_journal(state, &config, &event);
            } else {
                all_ok = false;
            }
        }
    }

    // 2) New setups. Track every enabled scenario, but only alert when the
    // trigger crosses on this bar or price enters the watch zone.
    for scenario in &scenarios.scenarios {
        if !scenario_alertable(scenario, &config) {
            continue;
        }
        if store.setups.contains_key(&scenario.id) || closed_this_tick.contains(&scenario.id) {
            continue;
        }

        let setup_id = format!("SETUP-{}-{}", candle.timestamp, scenario.id);
        let record = SetupRecord::new(setup_id, scenario.clone(), now, candle.timestamp);
        if record.is_invalidated(&candle) {
            continue;
        }

        if record.trigger_crossed(previous.as_ref(), &candle) {
            let key = event_key(&scenario.id, "triggered", candle.timestamp);
            if !store.has_sent(&key) {
                let (zone_low, zone_high) = zone_bounds(scenario, market);
                let (retest_low, retest_high) = retest_bounds(scenario, market);
                let flags = setup_flags(scenario, market, in_session);
                let confirmation = confirmation_text(scenario);
                let text = TelegramNotifier::format_triggered_message(
                    scenario,
                    candle.close,
                    in_session,
                    &flags,
                    zone_low,
                    zone_high,
                    retest_low,
                    retest_high,
                    &confirmation,
                    &context_line,
                );
                if send_setup_message(
                    &telegram,
                    &text,
                    &extra_chat_ids,
                    "TRIGGERED",
                    &record.setup_id,
                )
                .await
                {
                    let mut updated = record.clone();
                    updated.mark_triggered(now, candle.timestamp, candle.close);
                    store.setups.insert(scenario.id.clone(), updated.clone());
                    store.mark_sent(key, now);
                    let event = setup_event(
                        "setup_triggered",
                        &updated,
                        market,
                        &candle,
                        now,
                        in_session,
                        None,
                    );
                    record_journal(state, &config, &event);
                    continue;
                }
                all_ok = false;
            }
        } else if scenario.confidence >= config.action_watch_min_confidence
            && is_approaching(scenario, candle.close, watch_distance)
        {
            let Some(trigger_price) = scenario.trigger_price.filter(|value| value.is_finite())
            else {
                store.setups.insert(scenario.id.clone(), record);
                continue;
            };
            let pre_key = pre_alert_key(scenario, trigger_price, zone_size);
            let key = event_key(&scenario.id, "pre_alert", candle.timestamp);
            if pre_alert_cooldown_ok(&store, &pre_key, now, &config) && !store.has_sent(&key) {
                let distance = (candle.close - trigger_price).abs();
                let distance_atr = market.and_then(|market| {
                    let atr = market.h4_atr;
                    (atr.is_finite() && atr > 0.0).then(|| distance / atr)
                });
                let (zone_low, zone_high) = zone_bounds(scenario, market);
                let flags = setup_flags(scenario, market, in_session);
                let confirmation = confirmation_text(scenario);
                let text = TelegramNotifier::format_pre_alert_message(
                    scenario,
                    candle.close,
                    distance,
                    watch_distance,
                    distance_atr,
                    in_session,
                    &flags,
                    zone_low,
                    zone_high,
                    &confirmation,
                    &context_line,
                );
                if send_setup_message(
                    &telegram,
                    &text,
                    &extra_chat_ids,
                    "PRE_ALERT",
                    &record.setup_id,
                )
                .await
                {
                    let mut updated = record.clone();
                    updated.mark_pre_alert(now, candle.timestamp);
                    store.setups.insert(scenario.id.clone(), updated.clone());
                    store.mark_sent(key, now);
                    store.mark_pre_alert_sent(pre_key, now);
                    let event = setup_event(
                        "setup_pre_alert",
                        &updated,
                        market,
                        &candle,
                        now,
                        in_session,
                        None,
                    );
                    record_journal(state, &config, &event);
                    continue;
                }
                all_ok = false;
            }
        }

        // Track silently so a later bar can trigger/invalidate even when the
        // scenario was too far for a PRE_ALERT on this bar.
        if !store.setups.contains_key(&scenario.id) {
            store.setups.insert(scenario.id.clone(), record);
        }
    }

    store.prune(now);
    // Advance even when a send failed: otherwise a permanently invalid chat ID
    // would retry every 30 seconds forever. The event key stays unmarked, so a
    // still-valid setup can retry on a later alert lifecycle.
    store.last_bar_timestamp = Some(candle.timestamp);
    if !all_ok {
        warn!(
            "⚠️ [Setup Engine] Một số alert không gửi được; event chưa đánh dấu để có thể thử lại."
        );
    }
    {
        let mut last_sent = state.action_trigger_last_sent.write().await;
        prune_last_sent(&mut last_sent, MAX_ACTION_TRIGGER_DEDUP);
    }
    *state.setup_alerts.write().await = store;
    persist_state(state).await
}

fn record_journal(state: &AppState, config: &BotConfig, event: &TriggerEvent) {
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        event,
    );
}

/// Spawn the realtime setup-alert worker (30-second cadence).
pub fn spawn_action_trigger_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "👀 [Setup Engine] Worker PRE_ALERT/TRIGGERED/INVALIDATED đã khởi chạy (chu kỳ {} giây)...",
            ACTION_TRIGGER_INTERVAL_SECS
        );
        let mut interval = tokio::time::interval(Duration::from_secs(ACTION_TRIGGER_INTERVAL_SECS));
        loop {
            interval.tick().await;
            if let Err(err) = evaluate_once(&state).await {
                warn!("⚠️ [Setup Engine] Lỗi vòng quét: {}", err);
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

    fn config() -> BotConfig {
        BotConfig::default()
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
    fn watch_zone_key_changes_for_a_different_direction() {
        let scenario = scenario(true, Some(2701.0), "ABOVE");
        let mut opposite = scenario.clone();
        opposite.trigger_direction = "BELOW".to_string();

        assert_ne!(
            watch_zone_key(&scenario, 2701.0, 10.0),
            watch_zone_key(&opposite, 2701.0, 10.0)
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
    fn flags_include_counter_trend_high_risk_and_out_session() {
        let mut scenario = scenario(true, Some(2701.0), "ABOVE");
        scenario.bias = "BULLISH".to_string();
        scenario.macro_alignment = "COUNTER".to_string();
        let mut market = market();
        market.macro_bias = "BEARISH".to_string();
        market.rsi_m15 = Some(72.0);
        market.daily_change_pct = Some(3.0);

        let flags = setup_flags(&scenario, Some(&market), false);

        assert!(flags.iter().any(|flag| flag == "COUNTER_TREND"));
        assert!(flags.iter().any(|flag| flag == "HIGH_RISK"));
        assert!(flags.iter().any(|flag| flag == "OUT_SESSION"));
    }

    #[test]
    fn disabled_and_reference_scenarios_are_not_eligible() {
        let mut config = config();
        config.bearish_rejection_enabled = false;
        let mut rejection = scenario(true, Some(2701.0), "TOUCH");
        rejection.id = "bearish_rejection".to_string();
        assert!(!scenario_enabled(&rejection, &config));

        let mut bounce = scenario(true, Some(2701.0), "TOUCH");
        bounce.id = "bullish_bounce".to_string();
        assert!(!scenario_enabled(&bounce, &config));
    }

    #[test]
    fn structurally_invalid_scenario_with_levels_is_still_alertable() {
        let mut scenario = scenario(false, Some(2701.0), "ABOVE");
        assert!(!scenario.valid);
        assert!(scenario_alertable(&scenario, &config()));

        scenario.trigger_price = None;
        assert!(!scenario_alertable(&scenario, &config()));
    }

    #[test]
    fn invalid_structure_adds_a_visible_flag() {
        let scenario = scenario(false, Some(2701.0), "ABOVE");
        let flags = setup_flags(&scenario, Some(&market()), true);
        assert!(flags.iter().any(|flag| flag == "STRUCTURE_INVALID"));
    }

    #[test]
    fn approaching_uses_the_previous_bar_trigger() {
        let scenario = scenario(true, Some(2701.0), "ABOVE");
        assert!(is_approaching(&scenario, 2695.0, 10.0));
    }
}
