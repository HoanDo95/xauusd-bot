//! Retest shadow research worker.
//!
//! Watches the PRIMARY scenario after its trigger and records a **research**
//! retest event when price comes back to the broken level and rejects. It never
//! inserts a signal, consumes quota, calls AI or places an order. Telegram
//! messages are explicitly labelled `RESEARCH — RETEST SHADOW`.
//!
//! This mirrors `quantum_v3_htf_retest` in the backtest: wait up to 8 M15 bars,
//! enter at the broken level, SL beyond the retest candle high/low + 3 USD,
//! TP1/TP2 at 2R/3R.

use crate::action_alerts::candle_is_closed_and_fresh;
use crate::api::{current_scenarios, persist_state, AppState};
use crate::market_context::htf_allows_signal;
use crate::models::Candle;
use crate::scenarios::{scenario_triggered, Scenario};
use crate::storage::PersistentStore;
use crate::trigger_journal::{record, TriggerEvent};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration as StdDuration;
use tracing::{info, warn};

const RETEST_INTERVAL_SECS: u64 = 30;
const RETEST_WAIT_BARS: i64 = 8;
const RETEST_BUFFER_USD: f64 = 3.0;
const M15_MS: i64 = 15 * 60 * 1000;
const MIN_CONFIDENCE: u8 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetestWatch {
    pub setup_id: String,
    pub scenario: Scenario,
    pub broken_level: f64,
    pub original_stop_loss: f64,
    pub trigger_candle_timestamp: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RetestLevels {
    pub entry: f64,
    pub stop_loss: f64,
    pub take_profit_1: f64,
    pub take_profit_2: f64,
    pub risk_reward: f64,
}

impl RetestWatch {
    pub fn original_risk(&self) -> f64 {
        (self.broken_level - self.original_stop_loss).abs()
    }

    pub fn is_expired(&self, candle_timestamp: i64) -> bool {
        candle_timestamp.saturating_sub(self.trigger_candle_timestamp) > RETEST_WAIT_BARS * M15_MS
    }

    pub fn is_invalidated(&self, candle: &Candle) -> bool {
        let risk = self.original_risk();
        if !risk.is_finite() || risk <= 0.0 {
            return true;
        }
        match self.scenario.bias.trim().to_uppercase().as_str() {
            "BEARISH" => candle.low < self.broken_level - risk,
            "BULLISH" => candle.high > self.broken_level + risk,
            _ => true,
        }
    }

    /// Return retest levels when this candle retests the broken level and
    /// rejects. The caller must only pass candles **after** the trigger candle.
    pub fn retest_levels(&self, candle: &Candle) -> Option<RetestLevels> {
        let entry = self.broken_level;
        match self.scenario.bias.trim().to_uppercase().as_str() {
            "BEARISH" => {
                if candle.high >= entry && candle.close < entry && candle.close < candle.open {
                    let stop_loss = candle.high.max(entry) + RETEST_BUFFER_USD;
                    let risk = (entry - stop_loss).abs();
                    if !risk.is_finite() || risk <= 0.0 {
                        return None;
                    }
                    Some(RetestLevels {
                        entry,
                        stop_loss,
                        take_profit_1: entry - risk * 2.0,
                        take_profit_2: entry - risk * 3.0,
                        risk_reward: 2.0,
                    })
                } else {
                    None
                }
            }
            "BULLISH" => {
                if candle.low <= entry && candle.close > entry && candle.close > candle.open {
                    let stop_loss = candle.low.min(entry) - RETEST_BUFFER_USD;
                    let risk = (stop_loss - entry).abs();
                    if !risk.is_finite() || risk <= 0.0 {
                        return None;
                    }
                    Some(RetestLevels {
                        entry,
                        stop_loss,
                        take_profit_1: entry + risk * 2.0,
                        take_profit_2: entry + risk * 3.0,
                        risk_reward: 2.0,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

async fn record_watch_event(
    state: &AppState,
    event: &str,
    watch: &RetestWatch,
    candle: &Candle,
    levels: Option<&RetestLevels>,
    reason: &str,
) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new(event);
    entry.setup_id = Some(watch.setup_id.clone());
    entry.created_at = Some(Utc::now());
    entry.candle_timestamp = Some(candle.timestamp);
    entry.scenario_id = Some(watch.scenario.id.clone());
    entry.scenario_name = Some(watch.scenario.name.clone());
    entry.direction = Some(watch.scenario.trigger_direction.clone());
    entry.bias = Some(watch.scenario.bias.clone());
    entry.trigger = Some(watch.broken_level);
    entry.confidence = Some(watch.scenario.confidence);
    entry.valid = Some(watch.scenario.valid);
    entry.macro_alignment = Some(watch.scenario.macro_alignment.clone());
    entry.session = Some(
        if PersistentStore::is_within_golden_session(Utc::now(), true) {
            "IN"
        } else {
            "OUT"
        }
        .to_string(),
    );
    entry.source = Some("retest_shadow".to_string());
    entry.reason = Some(reason.to_string());
    if let Some(levels) = levels {
        entry.entry = Some(levels.entry);
        entry.stop_loss = Some(levels.stop_loss);
        entry.take_profit_1 = Some(levels.take_profit_1);
        entry.take_profit_2 = Some(levels.take_profit_2);
        entry.risk_points = Some((levels.entry - levels.stop_loss).abs());
        entry.close = Some(candle.close);
        entry.fill_price = Some(levels.entry);
    }
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

pub fn spawn_retest_shadow_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "🧪 [Retest Shadow] Worker đã khởi chạy (chu kỳ {}s, wait {} bars, buffer {} USD).",
            RETEST_INTERVAL_SECS, RETEST_WAIT_BARS, RETEST_BUFFER_USD
        );
        let mut interval = tokio::time::interval(StdDuration::from_secs(RETEST_INTERVAL_SECS));

        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.is_active || !config.retest_shadow_enabled {
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

            let active = state.retest_watch.read().await.clone();
            if let Some(watch) = active {
                // Never evaluate the trigger candle itself as a retest.
                if candle.timestamp <= watch.trigger_candle_timestamp {
                    continue;
                }
                if watch.is_expired(candle.timestamp) || watch.is_invalidated(&candle) {
                    let reason = if watch.is_expired(candle.timestamp) {
                        "expired"
                    } else {
                        "invalidated"
                    };
                    record_watch_event(
                        &state,
                        "retest_watch_closed",
                        &watch,
                        &candle,
                        None,
                        reason,
                    )
                    .await;
                    *state.retest_watch.write().await = None;
                    if let Err(err) = persist_state(&state).await {
                        warn!("⚠️ [Retest Shadow] Không lưu được clear watch: {}", err);
                    }
                    continue;
                }
                if let Some(levels) = watch.retest_levels(&candle) {
                    record_watch_event(
                        &state,
                        "retest_shadow_alert",
                        &watch,
                        &candle,
                        Some(&levels),
                        "retest_confirmed",
                    )
                    .await;
                    let telegram = state.telegram.read().await.clone();
                    if let Err(err) = telegram
                        .send_retest_shadow_alert(&watch.scenario, &levels, candle.close)
                        .await
                    {
                        warn!("⚠️ [Retest Shadow] Không gửi được Telegram: {}", err);
                    }
                    info!(
                        "🧪 [Retest Shadow] {} {} · retest {:.2} · SL {:.2} · TP1 {:.2}",
                        watch.scenario.id,
                        watch.scenario.trigger_direction,
                        levels.entry,
                        levels.stop_loss,
                        levels.take_profit_1
                    );
                    *state.retest_watch.write().await = None;
                    if let Err(err) = persist_state(&state).await {
                        warn!("⚠️ [Retest Shadow] Không lưu được clear watch: {}", err);
                    }
                }
                continue;
            }

            // No active watch: arm one when a valid PRIMARY triggers.
            let scenarios = current_scenarios(&state).await;
            let Some(primary) = scenarios
                .scenarios
                .iter()
                .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"))
            else {
                continue;
            };
            if !primary.valid
                || primary.confidence < MIN_CONFIDENCE
                || primary.macro_alignment.eq_ignore_ascii_case("COUNTER")
            {
                continue;
            }
            let side = match primary.bias.trim().to_uppercase().as_str() {
                "BULLISH" => "BUY",
                "BEARISH" => "SELL",
                _ => continue,
            };
            if let Some(context) = state.market_context.read().await.clone() {
                let (allowed, _) = htf_allows_signal(side, &context, true, true);
                if !allowed {
                    continue;
                }
                if side == "BUY"
                    && context
                        .rsi_h1
                        .as_ref()
                        .and_then(|rsi| rsi.value)
                        .is_some_and(|value| value < 30.0)
                {
                    continue;
                }
            }
            if !scenario_triggered(primary, &candle) {
                continue;
            }
            let Some(broken_level) = primary.entry.or(primary.trigger_price) else {
                continue;
            };
            let Some(original_stop_loss) = primary.stop_loss else {
                continue;
            };
            let watch = RetestWatch {
                setup_id: format!("RETEST-{}-{}", candle.timestamp, primary.id),
                scenario: primary.clone(),
                broken_level,
                original_stop_loss,
                trigger_candle_timestamp: candle.timestamp,
                created_at: Utc::now(),
            };
            *state.retest_watch.write().await = Some(watch.clone());
            record_watch_event(
                &state,
                "retest_watch_created",
                &watch,
                &candle,
                None,
                "trigger_confirmed_wait_retest",
            )
            .await;
            if let Err(err) = persist_state(&state).await {
                warn!("⚠️ [Retest Shadow] Không lưu được watch: {}", err);
            }
            info!(
                "🧪 [Retest Shadow] Armed {} {} · broken {:.2} · wait {} bars",
                watch.scenario.id, watch.scenario.trigger_direction, broken_level, RETEST_WAIT_BARS
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;

    fn candle(open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp: 1,
            open,
            high,
            low,
            close,
            volume: 1000.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    fn watch(bias: &str, level: f64, sl: f64) -> RetestWatch {
        RetestWatch {
            setup_id: "RETEST-TEST".to_string(),
            scenario: Scenario {
                id: "bearish_breakdown".to_string(),
                name: "Bearish breakdown".to_string(),
                bias: bias.to_string(),
                trigger: "test".to_string(),
                condition: "test".to_string(),
                entry: Some(level),
                stop_loss: Some(sl),
                take_profit_1: None,
                take_profit_2: None,
                risk_reward: Some(2.0),
                trigger_price: Some(level),
                trigger_direction: if bias == "BULLISH" {
                    "ABOVE".to_string()
                } else {
                    "BELOW".to_string()
                },
                trigger_timeframe: "M15".to_string(),
                invalidation: "test".to_string(),
                confidence: 70,
                macro_alignment: "ALIGNED".to_string(),
                valid: true,
                role: "PRIMARY".to_string(),
                priority: 70,
                notes: vec![],
            },
            broken_level: level,
            original_stop_loss: sl,
            trigger_candle_timestamp: 0,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn bearish_retest_requires_wick_and_bearish_close() {
        let watch = watch("BEARISH", 3800.0, 3810.0);
        assert!(watch
            .retest_levels(&candle(3799.0, 3801.0, 3790.0, 3797.0))
            .is_some());
        assert!(watch
            .retest_levels(&candle(3795.0, 3801.0, 3790.0, 3802.0))
            .is_none());
        assert!(watch
            .retest_levels(&candle(3802.0, 3803.0, 3799.0, 3801.0))
            .is_none());
    }

    #[test]
    fn bullish_retest_requires_dip_and_bullish_close() {
        let watch = watch("BULLISH", 3800.0, 3790.0);
        assert!(watch
            .retest_levels(&candle(3802.0, 3808.0, 3799.0, 3804.0))
            .is_some());
        assert!(watch
            .retest_levels(&candle(3805.0, 3808.0, 3799.0, 3798.0))
            .is_none());
    }

    #[test]
    fn expiry_and_invalidation_follow_original_risk() {
        let watch = watch("BEARISH", 3800.0, 3810.0);
        assert!(!watch.is_expired(8 * M15_MS));
        assert!(watch.is_expired(9 * M15_MS));
        assert!(watch.is_invalidated(&candle(3790.0, 3795.0, 3785.0, 3790.0)));
        assert!(!watch.is_invalidated(&candle(3795.0, 3800.0, 3792.0, 3795.0)));
    }
}
