//! Read-only Setup Inbox feed for the dashboard.
//!
//! Groups the current realtime setup state, recent setup journal events and the
//! latest signals so the UI can show one coherent inbox instead of a linear
//! Telegram stream. This endpoint never mutates state, sends Telegram or
//! consumes quota.

use super::AppState;
use crate::models::Signal;
use crate::storage::PersistentStore;
use crate::trigger_journal::TriggerEvent;
use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Serialize)]
struct SetupInboxItem {
    setup_id: String,
    scenario_id: String,
    name: String,
    role: String,
    bias: String,
    status: String,
    valid: bool,
    confidence: u8,
    trigger_price: Option<f64>,
    trigger_direction: String,
    entry: Option<f64>,
    stop_loss: Option<f64>,
    take_profit_1: Option<f64>,
    take_profit_2: Option<f64>,
    invalidation: String,
    distance_now: Option<f64>,
    distance_atr: Option<f64>,
    watch_distance: f64,
    stale: bool,
    session: String,
    flags: Vec<String>,
    pre_alerted: bool,
    last_event_bar: Option<i64>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct InboxSignal {
    id: String,
    source: String,
    side: String,
    entry: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    status: String,
    created_at: String,
    outcome_at: Option<String>,
    dedup_key: String,
    ai_verdict: Option<String>,
    ai_confidence: Option<f64>,
    telegram_message_id: Option<i64>,
    is_simulated: bool,
}

fn signal_source(signal: &Signal) -> &'static str {
    if signal.is_simulated {
        return "SIMULATION";
    }
    if signal
        .price_action_notes
        .starts_with("Realtime setup engine")
    {
        return "SETUP_ENGINE";
    }
    if signal.dedup_key.contains("SCENARIO_") {
        return "SCANNER_SCENARIO";
    }
    "SCANNER"
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub(super) async fn get_setup_inbox(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let now = Utc::now();
    let in_session = PersistentStore::is_within_split_session(now, config.session_filter_enabled);
    let last_bar = state
        .candles
        .read()
        .await
        .last()
        .map(|candle| candle.timestamp);
    let live_price = *state.live_price.read().await;
    let context = state.market_context.read().await.clone();
    let h4_atr = context
        .as_ref()
        .map(|context| context.h4.atr14)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(10.0);
    let watch_distance = (h4_atr * config.action_watch_distance_atr)
        .min(config.action_watch_max_distance_points)
        .max(1.0);
    let price = live_price
        .or_else(|| context.as_ref().map(|context| context.h1.close))
        .filter(|value| value.is_finite() && *value > 0.0);
    let rsi_m15 = context
        .as_ref()
        .and_then(|context| context.rsi_m15.as_ref())
        .and_then(|snapshot| snapshot.value);
    let daily_change = price
        .zip(context.as_ref().map(|context| context.d1.prev_close))
        .and_then(|(price, prev_close)| {
            (prev_close.is_finite() && prev_close > 0.0)
                .then(|| (price - prev_close) / prev_close * 100.0)
        });

    let setup_store = state.setup_alerts.read().await.clone();
    let mut setups = setup_store
        .setups
        .values()
        .map(|setup| {
            let scenario = &setup.scenario;
            let distance = price
                .zip(scenario.trigger_price)
                .map(|(price, trigger)| (price - trigger).abs());
            let distance_atr = distance.map(|distance| round2(distance / h4_atr));
            let stale = setup.status != "TRIGGERED"
                && distance.is_some_and(|distance| distance > watch_distance * 2.0);
            let mut flags = Vec::new();
            if scenario.macro_alignment.eq_ignore_ascii_case("COUNTER") {
                flags.push("COUNTER_TREND".to_string());
            }
            if !scenario.valid {
                flags.push("STRUCTURE_INVALID".to_string());
            }
            if !in_session {
                flags.push("OUT_SESSION".to_string());
            }
            let high_risk = match scenario.bias.trim().to_uppercase().as_str() {
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
            SetupInboxItem {
                setup_id: setup.setup_id.clone(),
                scenario_id: scenario.id.clone(),
                name: scenario.name.clone(),
                role: scenario.role.clone(),
                bias: scenario.bias.clone(),
                status: setup.status.clone(),
                valid: scenario.valid,
                confidence: scenario.confidence,
                trigger_price: scenario.trigger_price.map(round2),
                trigger_direction: scenario.trigger_direction.clone(),
                entry: scenario.entry.map(round2),
                stop_loss: scenario.stop_loss.map(round2),
                take_profit_1: scenario.take_profit_1.map(round2),
                take_profit_2: scenario.take_profit_2.map(round2),
                invalidation: scenario.invalidation.clone(),
                distance_now: distance.map(round2),
                distance_atr,
                watch_distance: round2(watch_distance),
                stale,
                session: if in_session { "IN" } else { "OUT" }.to_string(),
                flags,
                pre_alerted: setup.pre_alerted,
                last_event_bar: setup.last_event_bar,
                created_at: setup.created_at.to_rfc3339(),
                updated_at: setup.updated_at.to_rfc3339(),
            }
        })
        .collect::<Vec<_>>();
    setups.sort_by(|left, right| {
        left.stale
            .cmp(&right.stale)
            .then_with(|| left.scenario_id.cmp(&right.scenario_id))
    });

    let signals = state.signals.read().await;
    let inbox_signals = signals
        .iter()
        .take(40)
        .map(|signal| InboxSignal {
            id: signal.id.clone(),
            source: signal_source(signal).to_string(),
            side: signal.signal_type.as_str().to_string(),
            entry: round2(signal.entry_price),
            stop_loss: round2(signal.stop_loss),
            take_profit_1: round2(signal.take_profit_1),
            take_profit_2: round2(signal.take_profit_2),
            status: format!("{:?}", signal.status),
            created_at: signal.created_at.to_rfc3339(),
            outcome_at: signal.outcome_at.map(|at| at.to_rfc3339()),
            dedup_key: signal.dedup_key.clone(),
            ai_verdict: signal.ai_verdict.clone(),
            ai_confidence: signal.ai_confidence,
            telegram_message_id: signal.telegram_message_id,
            is_simulated: signal.is_simulated,
        })
        .collect::<Vec<_>>();

    let events = state
        .trigger_journal
        .load(Some(300))
        .into_iter()
        .filter(|event| {
            event.event.starts_with("setup_")
                || event.event == "scenario_observed"
                || event.event == "triggered"
        })
        .rev()
        .take(120)
        .collect::<Vec<TriggerEvent>>();

    Json(json!({
        "generatedAt": now.to_rfc3339(),
        "lastBar": last_bar,
        "price": price.map(round2),
        "session": if in_session { "IN" } else { "OUT" },
        "quietMode": config.action_quiet_mode,
        "watchDistance": round2(watch_distance),
        "h4Atr": round2(h4_atr),
        "setups": setups,
        "signals": inbox_signals,
        "events": events,
    }))
}
