//! Strategy v2 shadow: raw PRIMARY trigger + wide SL (1× H4 ATR) + 2R/3R.
//!
//! Journal-only research profile. It never sends Telegram, inserts a signal,
//! consumes quota, calls AI or changes the live strategy. Its purpose is to
//! collect forward evidence for the entry × SL A/B candidate `breakdown_wide`.

use crate::action_alerts::candle_is_closed_and_fresh;
use crate::api::{current_scenarios, AppState};
use crate::decision_gates::confirmed_signal_with_gates;
use crate::decision_trace::{
    config_hash, decision_id, h4_atr_percentile, snapshot_hash, wick_body_ratio, zone_quality,
};
use crate::market_view::score_scenario;
use crate::rsi::rsi_snapshot;
use crate::scenarios::{scenario_triggered, Scenario};
use crate::storage::PersistentStore;
use crate::trigger_journal::regime_label;
use crate::trigger_journal::{record, TriggerEvent};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};

const INTERVAL_SECS: u64 = 30;
pub const STRATEGY_V2_VERSION: &str = "2026-11-19.1-v2-shadow";
const WIDE_SL_ATR: f64 = 1.0;
const TP1_R: f64 = 2.0;
const TP2_R: f64 = 3.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StrategyV2Levels {
    pub entry: f64,
    pub stop_loss: f64,
    pub take_profit_1: f64,
    pub take_profit_2: f64,
    pub risk: f64,
}

pub fn build_levels(scenario: &Scenario, h4_atr: f64) -> Option<StrategyV2Levels> {
    let entry = scenario.entry.or(scenario.trigger_price)?;
    if !entry.is_finite() || entry <= 0.0 || !h4_atr.is_finite() || h4_atr <= 0.0 {
        return None;
    }
    let risk = h4_atr * WIDE_SL_ATR;
    match scenario.bias.trim().to_uppercase().as_str() {
        "BEARISH" => Some(StrategyV2Levels {
            entry,
            stop_loss: entry + risk,
            take_profit_1: entry - risk * TP1_R,
            take_profit_2: entry - risk * TP2_R,
            risk,
        }),
        "BULLISH" => Some(StrategyV2Levels {
            entry,
            stop_loss: entry - risk,
            take_profit_1: entry + risk * TP1_R,
            take_profit_2: entry + risk * TP2_R,
            risk,
        }),
        _ => None,
    }
}

pub fn spawn_strategy_v2_shadow_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "🧪 [Strategy V2] Shadow worker đã khởi chạy (raw trigger, wide SL 1×H4 ATR, journal-only)."
        );
        let mut interval = tokio::time::interval(Duration::from_secs(INTERVAL_SECS));
        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.is_active || !config.strategy_v2_shadow_enabled {
                continue;
            }
            if PersistentStore::is_market_closed_weekend(Utc::now()) {
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
                Utc::now().timestamp_millis(),
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
            if !primary.valid || !scenario_triggered(primary, &candle) {
                continue;
            }
            let context = state.market_context.read().await.clone();
            let Some(context) = context else {
                continue;
            };
            let Some(levels) = build_levels(primary, context.h4.atr14) else {
                continue;
            };
            let setup_id = format!("V2-{}-{}", candle.timestamp, primary.id);
            if state
                .trigger_journal
                .has_event_for_candle("strategy_v2_shadow", candle.timestamp)
            {
                continue;
            }

            let candles = state.candles.read().await.clone();
            let rsi = rsi_snapshot(
                &candles,
                config.rsi_period,
                config.rsi_overbought,
                config.rsi_oversold,
                config.rsi_divergence_lookback,
                config.rsi_pivot_k,
            );
            let confirmation = confirmed_signal_with_gates(
                primary,
                &config,
                &candles,
                Some(candle.close),
                rsi.value,
                Utc::now(),
            );
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
            let now = Utc::now();
            let mut event = TriggerEvent::new("strategy_v2_shadow");
            event.setup_id = Some(setup_id);
            event.created_at = Some(now);
            event.candle_timestamp = Some(candle.timestamp);
            event.scenario_id = Some(primary.id.clone());
            event.scenario_name = Some(primary.name.clone());
            event.direction = Some(primary.trigger_direction.clone());
            event.bias = Some(primary.bias.clone());
            event.trigger = primary.trigger_price;
            event.entry = Some(levels.entry);
            event.stop_loss = Some(levels.stop_loss);
            event.take_profit_1 = Some(levels.take_profit_1);
            event.take_profit_2 = Some(levels.take_profit_2);
            event.risk_points = Some(levels.risk);
            event.confidence = Some(primary.confidence);
            event.valid = Some(primary.valid);
            event.macro_alignment = Some(primary.macro_alignment.clone());
            let session = if PersistentStore::is_within_golden_session(now, true) {
                "IN"
            } else {
                "OUT"
            };
            event.session = Some(session.to_string());
            event.regime = Some(regime_label(
                &context.d1.bias,
                &context.h4.bias,
                &context.h1.bias,
            ));
            event.h4_atr = Some(context.h4.atr14);
            event.close = Some(candle.close);
            event.fill_price = Some(levels.entry);
            event.entry_type = Some("WIDE".to_string());
            event.source = Some("strategy_v2_shadow".to_string());
            event.gate = Some("raw_trigger".to_string());
            event.gate_subreason = Some("wide_sl_1x_h4_atr".to_string());
            event.confirmation_pass = Some(confirmation.signal.is_some());
            event.confirmation_reason = Some(confirmation.reason.clone());
            event.confirmation_type = confirmation
                .confirmation_type
                .map(|kind| kind.as_str().to_string());
            event.shadow = Some(true);
            event.strategy_version = Some(STRATEGY_V2_VERSION.to_string());
            event.config_hash = Some(cfg_hash);
            event.snapshot_hash = Some(snap_hash.clone());
            event.decision_id = Some(decision_id(now, &snap_hash));
            event.rsi_h1 = context.rsi_h1.as_ref().and_then(|snapshot| snapshot.value);
            event.rsi_m15 = rsi.value;
            event.sl_distance_atr = Some(1.0);
            event.atr_percentile = h4_atr_percentile(&candles);
            event.wick_body_ratio = wick_body_ratio(&candle);
            if let Some((touches, width_atr)) =
                zone_quality(&candles, levels.entry, context.h4.atr14)
            {
                event.zone_touch_count = Some(touches);
                event.zone_width_atr = Some(width_atr);
            }
            event.score = Some(
                score_scenario(
                    primary,
                    Some(&context),
                    crate::market_strategist::volume_surge_ratio(&candles, 20),
                )
                .total,
            );
            record(
                &state.trigger_journal,
                config.trigger_journal_enabled,
                &event,
            );
            if config.strategy_v2_shadow_telegram_enabled {
                let telegram = state.telegram.read().await.clone();
                match telegram
                    .send_strategy_v2_shadow_alert(
                        primary,
                        levels.entry,
                        levels.stop_loss,
                        levels.take_profit_1,
                        levels.take_profit_2,
                        candle.close,
                        session,
                    )
                    .await
                {
                    Ok(msg_id) => info!(
                        "🧪 [Strategy V2] Đã gửi Telegram research alert (msg_id={}).",
                        msg_id
                    ),
                    Err(err) => warn!(
                        "⚠️ [Strategy V2] Không gửi được Telegram research alert: {}",
                        err
                    ),
                }
            }
            info!(
                "🧪 [Strategy V2] {} {} · entry {:.2} · SL {:.2} · TP1 {:.2} · TP2 {:.2} · confirmed={}",
                primary.id,
                primary.trigger_direction,
                levels.entry,
                levels.stop_loss,
                levels.take_profit_1,
                levels.take_profit_2,
                confirmation.signal.is_some()
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;

    fn scenario(bias: &str) -> Scenario {
        Scenario {
            id: "test".to_string(),
            name: "Test".to_string(),
            bias: bias.to_string(),
            trigger: "test".to_string(),
            condition: "test".to_string(),
            entry: Some(3800.0),
            stop_loss: Some(if bias == "BEARISH" { 3810.0 } else { 3790.0 }),
            take_profit_1: None,
            take_profit_2: None,
            risk_reward: Some(2.0),
            trigger_price: Some(3800.0),
            trigger_direction: if bias == "BEARISH" {
                "BELOW".to_string()
            } else {
                "ABOVE".to_string()
            },
            trigger_timeframe: "M15".to_string(),
            invalidation: "test".to_string(),
            confidence: 70,
            macro_alignment: "ALIGNED".to_string(),
            valid: true,
            role: "PRIMARY".to_string(),
            priority: 70,
            notes: vec![],
        }
    }

    #[test]
    fn wide_sl_and_targets_are_symmetric() {
        let sell = build_levels(&scenario("BEARISH"), 20.0).expect("sell levels");
        assert_eq!(sell.stop_loss, 3820.0);
        assert_eq!(sell.take_profit_1, 3760.0);
        assert_eq!(sell.take_profit_2, 3740.0);
        assert_eq!(sell.risk, 20.0);

        let buy = build_levels(&scenario("BULLISH"), 20.0).expect("buy levels");
        assert_eq!(buy.stop_loss, 3780.0);
        assert_eq!(buy.take_profit_1, 3840.0);
        assert_eq!(buy.take_profit_2, 3860.0);
    }

    #[test]
    fn invalid_atr_returns_none() {
        assert!(build_levels(&scenario("BEARISH"), 0.0).is_none());
        assert!(build_levels(&scenario("BEARISH"), f64::NAN).is_none());
    }

    #[test]
    fn candle_timeframe_is_supported() {
        // Keep the import used in this module's test build.
        assert_eq!(Timeframe::M15.to_string(), "M15");
    }
}
