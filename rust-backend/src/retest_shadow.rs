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

/// A retest trade that was alerted and is being tracked to outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetestTrade {
    pub setup_id: String,
    pub scenario_id: String,
    pub bias: String,
    pub confidence: u8,
    /// BUY | SELL
    pub direction: String,
    pub entry: f64,
    pub stop_loss: f64,
    pub take_profit_1: f64,
    pub take_profit_2: f64,
    pub risk: f64,
    pub created_at: DateTime<Utc>,
    pub entry_candle_timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct RetestOutcome {
    pub outcome: &'static str,
    pub r_multiple: f64,
}

impl RetestTrade {
    pub fn from_watch(
        watch: &RetestWatch,
        direction: String,
        levels: RetestLevels,
        entry_candle_timestamp: i64,
    ) -> Option<Self> {
        let risk = (levels.entry - levels.stop_loss).abs();
        if !risk.is_finite() || risk <= 0.0 {
            return None;
        }
        Some(Self {
            setup_id: watch.setup_id.clone(),
            scenario_id: watch.scenario.id.clone(),
            bias: watch.scenario.bias.clone(),
            confidence: watch.scenario.confidence,
            direction,
            entry: levels.entry,
            stop_loss: levels.stop_loss,
            take_profit_1: levels.take_profit_1,
            take_profit_2: levels.take_profit_2,
            risk,
            created_at: Utc::now(),
            entry_candle_timestamp,
        })
    }

    pub fn is_expired(&self, candle_timestamp: i64) -> bool {
        candle_timestamp.saturating_sub(self.entry_candle_timestamp) >= 96 * M15_MS
    }

    pub fn evaluate(&self, candle: &Candle) -> Option<RetestOutcome> {
        match self.direction.as_str() {
            "SELL" => {
                if candle.high >= self.stop_loss {
                    Some(RetestOutcome {
                        outcome: "SL",
                        r_multiple: -1.0,
                    })
                } else if candle.low <= self.take_profit_2 {
                    Some(RetestOutcome {
                        outcome: "TP2",
                        r_multiple: 3.0,
                    })
                } else if candle.low <= self.take_profit_1 {
                    Some(RetestOutcome {
                        outcome: "TP1",
                        r_multiple: 2.0,
                    })
                } else {
                    None
                }
            }
            "BUY" => {
                if candle.low <= self.stop_loss {
                    Some(RetestOutcome {
                        outcome: "SL",
                        r_multiple: -1.0,
                    })
                } else if candle.high >= self.take_profit_2 {
                    Some(RetestOutcome {
                        outcome: "TP2",
                        r_multiple: 3.0,
                    })
                } else if candle.high >= self.take_profit_1 {
                    Some(RetestOutcome {
                        outcome: "TP1",
                        r_multiple: 2.0,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn timeout_r(&self, close: f64) -> f64 {
        if self.risk <= 0.0 {
            return 0.0;
        }
        match self.direction.as_str() {
            "SELL" => (self.entry - close) / self.risk,
            "BUY" => (close - self.entry) / self.risk,
            _ => 0.0,
        }
    }
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
                        take_profit_1: entry - (risk * 2.0 + 0.01),
                        take_profit_2: entry - (risk * 3.0 + 0.01),
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
                        take_profit_1: entry + (risk * 2.0 + 0.01),
                        take_profit_2: entry + (risk * 3.0 + 0.01),
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

async fn record_outcome_event(
    state: &AppState,
    trade: &RetestTrade,
    candle: &Candle,
    outcome: &str,
    r_multiple: f64,
) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new("retest_outcome");
    entry.setup_id = Some(trade.setup_id.clone());
    entry.scenario_id = Some(trade.scenario_id.clone());
    entry.bias = Some(trade.bias.clone());
    entry.confidence = Some(trade.confidence);
    entry.created_at = Some(Utc::now());
    entry.candle_timestamp = Some(candle.timestamp);
    entry.direction = Some(trade.direction.clone());
    entry.entry = Some(trade.entry);
    entry.stop_loss = Some(trade.stop_loss);
    entry.take_profit_1 = Some(trade.take_profit_1);
    entry.take_profit_2 = Some(trade.take_profit_2);
    entry.risk_points = Some(trade.risk);
    entry.close = Some(candle.close);
    entry.r_multiple = Some(r_multiple);
    entry.bars_open = Some(
        candle
            .timestamp
            .saturating_sub(trade.entry_candle_timestamp)
            / M15_MS,
    );
    entry.reason = Some(outcome.to_string());
    entry.source = Some("retest_shadow".to_string());
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

            let active_trade = state.active_retest_trade.read().await.clone();
            if let Some(trade) = active_trade {
                if candle.timestamp > trade.entry_candle_timestamp {
                    if let Some(outcome) = trade.evaluate(&candle) {
                        record_outcome_event(
                            &state,
                            &trade,
                            &candle,
                            outcome.outcome,
                            outcome.r_multiple,
                        )
                        .await;
                        info!(
                            "🧪 [Retest Shadow] Outcome {} {} · R {:+.2} · bars {}",
                            trade.setup_id,
                            outcome.outcome,
                            outcome.r_multiple,
                            (candle.timestamp - trade.entry_candle_timestamp) / M15_MS
                        );
                        *state.active_retest_trade.write().await = None;
                        if let Err(err) = persist_state(&state).await {
                            warn!("⚠️ [Retest Shadow] Không lưu được outcome: {}", err);
                        }
                        continue;
                    }
                    if trade.is_expired(candle.timestamp) {
                        let r = trade.timeout_r(candle.close);
                        record_outcome_event(&state, &trade, &candle, "TIMEOUT", r).await;
                        info!(
                            "🧪 [Retest Shadow] Outcome {} TIMEOUT · R {:+.2}",
                            trade.setup_id, r
                        );
                        *state.active_retest_trade.write().await = None;
                        if let Err(err) = persist_state(&state).await {
                            warn!("⚠️ [Retest Shadow] Không lưu được timeout: {}", err);
                        }
                        continue;
                    }
                }
                // One open research trade at a time; do not arm another watch.
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
                    let direction = if watch.scenario.bias.trim().eq_ignore_ascii_case("BEARISH") {
                        "SELL"
                    } else {
                        "BUY"
                    };
                    if let Some(trade) = RetestTrade::from_watch(
                        &watch,
                        direction.to_string(),
                        levels,
                        candle.timestamp,
                    ) {
                        // Match the backtest: the retest candle itself can reach
                        // TP1/TP2 (SL is beyond its wick + buffer, so it cannot
                        // hit SL on this candle).
                        if let Some(outcome) = trade.evaluate(&candle) {
                            record_outcome_event(
                                &state,
                                &trade,
                                &candle,
                                outcome.outcome,
                                outcome.r_multiple,
                            )
                            .await;
                            info!(
                                "🧪 [Retest Shadow] Outcome {} {} · R {:+.2} · bars 0",
                                trade.setup_id, outcome.outcome, outcome.r_multiple
                            );
                        } else {
                            *state.active_retest_trade.write().await = Some(trade);
                        }
                    }
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
    fn retest_tp1_keeps_min_rr_after_price_rounding() {
        let bearish = watch("BEARISH", 3800.0, 3810.0);
        let levels = bearish
            .retest_levels(&candle(3799.0, 3801.0, 3790.0, 3797.0))
            .expect("bearish retest");
        let risk = (levels.entry - levels.stop_loss).abs();
        let reward = (levels.entry - levels.take_profit_1).abs();
        assert!(reward / risk >= 2.0);

        let bullish = watch("BULLISH", 3800.0, 3790.0);
        let levels = bullish
            .retest_levels(&candle(3802.0, 3808.0, 3799.0, 3804.0))
            .expect("bullish retest");
        let risk = (levels.entry - levels.stop_loss).abs();
        let reward = (levels.take_profit_1 - levels.entry).abs();
        assert!(reward / risk >= 2.0);
    }

    #[test]
    fn expiry_and_invalidation_follow_original_risk() {
        let watch = watch("BEARISH", 3800.0, 3810.0);
        assert!(!watch.is_expired(8 * M15_MS));
        assert!(watch.is_expired(9 * M15_MS));
        assert!(watch.is_invalidated(&candle(3790.0, 3795.0, 3785.0, 3790.0)));
        assert!(!watch.is_invalidated(&candle(3795.0, 3800.0, 3792.0, 3795.0)));
    }

    fn trade(direction: &str) -> RetestTrade {
        RetestTrade {
            setup_id: "RETEST-TEST".to_string(),
            scenario_id: "bearish_breakdown".to_string(),
            bias: if direction == "SELL" {
                "BEARISH".to_string()
            } else {
                "BULLISH".to_string()
            },
            confidence: 70,
            direction: direction.to_string(),
            entry: 3800.0,
            stop_loss: if direction == "SELL" { 3810.0 } else { 3790.0 },
            take_profit_1: if direction == "SELL" { 3780.0 } else { 3820.0 },
            take_profit_2: if direction == "SELL" { 3770.0 } else { 3830.0 },
            risk: 10.0,
            created_at: Utc::now(),
            entry_candle_timestamp: 0,
        }
    }

    #[test]
    fn trade_levels_reject_zero_risk() {
        let watch = watch("BEARISH", 3800.0, 3810.0);
        let levels = RetestLevels {
            entry: 3800.0,
            stop_loss: 3800.0,
            take_profit_1: 3780.0,
            take_profit_2: 3770.0,
            risk_reward: 2.0,
        };
        assert!(RetestTrade::from_watch(&watch, "SELL".to_string(), levels, 0).is_none());
    }

    #[test]
    fn sell_trade_prefers_stop_when_candle_hits_both_sides() {
        let trade = trade("SELL");
        let outcome = trade
            .evaluate(&candle(3800.0, 3811.0, 3769.0, 3775.0))
            .expect("outcome");
        assert_eq!(outcome.outcome, "SL");
        assert_eq!(outcome.r_multiple, -1.0);
    }

    #[test]
    fn sell_trade_hits_tp1_then_tp2() {
        let trade = trade("SELL");
        let tp1 = trade
            .evaluate(&candle(3800.0, 3801.0, 3779.0, 3781.0))
            .expect("tp1");
        assert_eq!(tp1.outcome, "TP1");
        assert_eq!(tp1.r_multiple, 2.0);
        let tp2 = trade
            .evaluate(&candle(3800.0, 3801.0, 3769.0, 3771.0))
            .expect("tp2");
        assert_eq!(tp2.outcome, "TP2");
        assert_eq!(tp2.r_multiple, 3.0);
        assert!(trade
            .evaluate(&candle(3800.0, 3801.0, 3781.0, 3782.0))
            .is_none());
    }

    #[test]
    fn buy_trade_hits_stop_and_targets() {
        let trade = trade("BUY");
        let sl = trade
            .evaluate(&candle(3800.0, 3831.0, 3789.0, 3795.0))
            .expect("sl");
        assert_eq!(sl.outcome, "SL");
        let tp2 = trade
            .evaluate(&candle(3800.0, 3831.0, 3799.0, 3825.0))
            .expect("tp2");
        assert_eq!(tp2.outcome, "TP2");
    }

    #[test]
    fn timeout_r_is_signed_by_direction() {
        let sell = trade("SELL");
        assert!((sell.timeout_r(3795.0) - 0.5).abs() < 1e-9);
        assert!((sell.timeout_r(3805.0) + 0.5).abs() < 1e-9);
        let buy = trade("BUY");
        assert!((buy.timeout_r(3805.0) - 0.5).abs() < 1e-9);
        assert!((buy.timeout_r(3795.0) + 0.5).abs() < 1e-9);
    }

    #[test]
    fn trade_timeout_is_96_bars_after_entry() {
        let trade = trade("SELL");
        assert!(!trade.is_expired(95 * M15_MS));
        assert!(trade.is_expired(96 * M15_MS));
    }
}
