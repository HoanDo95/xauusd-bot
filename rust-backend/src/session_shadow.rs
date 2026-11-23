//! Session shadow: the live confirmed-scenario signal, recorded in all sessions.
//!
//! Journal-only research profile. It never sends Telegram, inserts a signal,
//! consumes quota, calls AI or changes the live session filter. Its purpose is
//! to compare the current 13:00–23:00 VN session against a split or full-24h
//! session with forward evidence.

use crate::action_alerts::candle_is_closed_and_fresh;
use crate::api::{current_scenarios, AppState};
use crate::decision_gates::confirmed_signal_with_gates;
use crate::decision_trace::{
    config_hash, decision_id, h4_atr_percentile, snapshot_hash, wick_body_ratio, zone_quality,
};
use crate::market_view::score_scenario;
use crate::rsi::rsi_snapshot;
use crate::scenario_signal::best_confirmed_scenario;
use crate::storage::PersistentStore;
use crate::trigger_journal::regime_label;
use crate::trigger_journal::{record, TriggerEvent};
use chrono::{DateTime, Utc};
use std::time::Duration;
use tracing::{info, warn};

const INTERVAL_SECS: u64 = 30;
pub const SESSION_SHADOW_VERSION: &str = "2026-11-19.1-session-shadow";

/// Label the fixed 13:00–23:00 VN window independently of the live session
/// flag, so the shadow keeps a stable research baseline even though live moved
/// to the split session on 2026-11-23.
pub fn session_label(now: DateTime<Utc>) -> &'static str {
    if PersistentStore::is_within_golden_session(now, true) {
        "IN"
    } else {
        "OUT"
    }
}

pub fn spawn_session_shadow_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "🧭 [Session Shadow] Worker đã khởi chạy (confirmed scenario, mọi session, journal-only)."
        );
        let mut interval = tokio::time::interval(Duration::from_secs(INTERVAL_SECS));
        let mut last_evaluated_candle: Option<i64> = None;
        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.is_active || !config.session_shadow_enabled {
                continue;
            }
            let now = Utc::now();
            if PersistentStore::is_market_closed_weekend(now) {
                continue;
            }
            let candle = {
                let candles = state.candles.read().await;
                candles.last().cloned()
            };
            let Some(candle) = candle else {
                continue;
            };
            if !candle_is_closed_and_fresh(
                &candle,
                now.timestamp_millis(),
                config.max_candle_age_secs,
            ) {
                continue;
            }
            if last_evaluated_candle == Some(candle.timestamp) {
                continue;
            }
            let live_price = *state.live_price.read().await;
            if live_price.is_none() {
                continue;
            }
            // Mark the candle only after the live price is available; a stale
            // feed should not consume the single evaluation for this candle.
            last_evaluated_candle = Some(candle.timestamp);

            let active_setup = state.active_watch_setup.read().await.clone();
            let scenario =
                if let Some(setup) = active_setup.filter(|setup| setup.is_triggered(&candle)) {
                    Some(setup.scenario)
                } else {
                    let scenarios = current_scenarios(&state).await;
                    best_confirmed_scenario(&scenarios, &candle).cloned()
                };
            let Some(scenario) = scenario else {
                continue;
            };

            let candles = state.candles.read().await.clone();
            let rsi = rsi_snapshot(
                &candles,
                config.rsi_period,
                config.rsi_overbought,
                config.rsi_oversold,
                config.rsi_divergence_lookback,
                config.rsi_pivot_k,
            );
            let evaluation = confirmed_signal_with_gates(
                &scenario, &config, &candles, live_price, rsi.value, now,
            );
            let Some(signal) = evaluation.signal else {
                continue;
            };
            if state
                .trigger_journal
                .has_event_for_candle("session_shadow", candle.timestamp)
            {
                continue;
            }
            let context = state.market_context.read().await.clone();
            let Some(context) = context else {
                continue;
            };
            let cfg_hash = config_hash(&config);
            let macro_flow = state.macro_flow.read().await.clone();
            let news_events = state.news_filter.read().await.calendar_events.clone();
            let snap_hash = snapshot_hash(
                &cfg_hash,
                &candles,
                Some(&context),
                macro_flow.as_ref(),
                &news_events,
            );
            let setup_id = format!("SS-{}-{}", candle.timestamp, scenario.id);
            let session = session_label(now);
            let mut event = TriggerEvent::new("session_shadow");
            event.setup_id = Some(setup_id);
            event.created_at = Some(now);
            event.candle_timestamp = Some(candle.timestamp);
            event.scenario_id = Some(scenario.id.clone());
            event.scenario_name = Some(scenario.name.clone());
            event.direction = Some(signal.signal_type.as_str().to_string());
            event.bias = Some(scenario.bias.clone());
            event.trigger = scenario.trigger_price;
            event.entry = Some(signal.entry_price);
            event.stop_loss = Some(signal.stop_loss);
            event.take_profit_1 = Some(signal.take_profit_1);
            event.take_profit_2 = Some(signal.take_profit_2);
            event.risk_points = Some(signal.risk_points);
            event.confidence = Some(scenario.confidence);
            event.valid = Some(scenario.valid);
            event.macro_alignment = Some(scenario.macro_alignment.clone());
            event.session = Some(session.to_string());
            event.regime = Some(regime_label(
                &context.d1.bias,
                &context.h4.bias,
                &context.h1.bias,
            ));
            event.h4_atr = Some(context.h4.atr14);
            event.close = Some(candle.close);
            event.fill_price = Some(signal.entry_price);
            event.entry_type = Some(signal.entry_type.as_str().to_string());
            event.source = Some("session_shadow".to_string());
            event.gate = Some("session_shadow".to_string());
            event.gate_subreason = Some("all_sessions_confirmed".to_string());
            event.confirmation_pass = Some(true);
            event.confirmation_reason = Some(evaluation.reason.clone());
            event.confirmation_type = evaluation
                .confirmation_type
                .map(|kind| kind.as_str().to_string());
            event.shadow = Some(true);
            event.strategy_version = Some(SESSION_SHADOW_VERSION.to_string());
            event.config_hash = Some(cfg_hash);
            event.snapshot_hash = Some(snap_hash.clone());
            event.decision_id = Some(decision_id(now, &snap_hash));
            event.rsi_h1 = context.rsi_h1.as_ref().and_then(|snapshot| snapshot.value);
            event.rsi_m15 = rsi.value;
            event.sl_distance_atr =
                (context.h4.atr14 > 0.0).then_some(signal.risk_points / context.h4.atr14);
            event.atr_percentile = h4_atr_percentile(&candles);
            event.wick_body_ratio = wick_body_ratio(&candle);
            if let Some((touches, width_atr)) =
                zone_quality(&candles, signal.entry_price, context.h4.atr14)
            {
                event.zone_touch_count = Some(touches);
                event.zone_width_atr = Some(width_atr);
            }
            event.score = Some(
                score_scenario(
                    &scenario,
                    Some(&context),
                    crate::market_strategist::volume_surge_ratio(&candles, 20),
                )
                .total,
            );
            event.notes = Some(
                "session shadow: confirmed scenario, all sessions; no AI/news/quota/session gate"
                    .to_string(),
            );
            record(
                &state.trigger_journal,
                config.trigger_journal_enabled,
                &event,
            );
            if config.session_shadow_telegram_enabled {
                let telegram = state.telegram.read().await.clone();
                match telegram
                    .send_session_shadow_alert(&scenario, &signal, candle.close, session)
                    .await
                {
                    Ok(msg_id) => info!(
                        "🧭 [Session Shadow] Đã gửi Telegram research alert (msg_id={}).",
                        msg_id
                    ),
                    Err(err) => warn!(
                        "⚠️ [Session Shadow] Không gửi được Telegram research alert: {}",
                        err
                    ),
                }
            }
            info!(
                "🧭 [Session Shadow] {} {} · {} · entry {:.2} · SL {:.2} · TP1 {:.2} · TP2 {:.2}",
                scenario.id,
                signal.signal_type.as_str(),
                session,
                signal.entry_price,
                signal.stop_loss,
                signal.take_profit_1,
                signal.take_profit_2
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap()
    }

    #[test]
    fn session_label_uses_fixed_vn_window() {
        // 06:00 UTC = 13:00 VN, inside.
        assert_eq!(session_label(utc(2026, 11, 19, 6, 0)), "IN");
        // 05:59 UTC = 12:59 VN, outside.
        assert_eq!(session_label(utc(2026, 11, 19, 5, 59)), "OUT");
        // 16:59 UTC = 23:59 VN, inside the live inclusive window.
        assert_eq!(session_label(utc(2026, 11, 19, 16, 59)), "IN");
        // 17:00 UTC = 00:00 VN, outside.
        assert_eq!(session_label(utc(2026, 11, 19, 17, 0)), "OUT");
    }
}
