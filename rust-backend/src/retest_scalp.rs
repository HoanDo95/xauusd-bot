//! Retest scalp research worker.
//!
//! Alert-only profile: after a valid PRIMARY trigger, wait up to 8 M15 bars
//! for a retest of the broken level, then publish a research alert with
//! structural SL (capped at `retest_scalp_max_sl_usd`), TP1/TP2 absolute
//! distances and a 50/50 partial plan. It never inserts a signal, consumes
//! quota, calls AI or auto-trades.
//!
//! The profile is deliberately separate from `retest_shadow` (2R/3R) so the
//! existing forward test is not contaminated.

use crate::action_alerts::candle_is_closed_and_fresh;
use crate::api::{current_scenarios, persist_state, AppState};
use crate::market_context::htf_allows_signal;
use crate::models::Candle;
use crate::retest_shadow::{RetestLevels, RetestWatch};
use crate::risk_manager::PositionSizer;
use crate::scenarios::{scenario_triggered, Scenario};
use crate::storage::PersistentStore;
use crate::trigger_journal::{record, TriggerEvent};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration as StdDuration;
use tracing::{info, warn};

const RETEST_SCALP_INTERVAL_SECS: u64 = 30;
const RETEST_SCALP_WAIT_BARS: i64 = 8;
const RETEST_SCALP_TIMEOUT_BARS: i64 = 96;
const M15_MS: i64 = 15 * 60 * 1000;
const MIN_CONFIDENCE: u8 = 60;
const TP1_WEIGHT: f64 = 0.5;
const TP2_WEIGHT: f64 = 0.5;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RetestScalpLevels {
    pub entry: f64,
    pub stop_loss: f64,
    pub take_profit_1: f64,
    pub take_profit_2: f64,
    pub risk: f64,
}

impl RetestScalpLevels {
    pub fn from_retest(
        levels: &RetestLevels,
        direction: &str,
        tp1_usd: f64,
        tp2_usd: f64,
    ) -> Option<Self> {
        let entry = levels.entry;
        let stop_loss = levels.stop_loss;
        let risk = (entry - stop_loss).abs();
        if !risk.is_finite() || risk <= 0.0 || !tp1_usd.is_finite() || !tp2_usd.is_finite() {
            return None;
        }
        let (take_profit_1, take_profit_2) = match direction {
            "SELL" => (entry - tp1_usd, entry - tp2_usd),
            "BUY" => (entry + tp1_usd, entry + tp2_usd),
            _ => return None,
        };
        Some(Self {
            entry,
            stop_loss,
            take_profit_1,
            take_profit_2,
            risk,
        })
    }
}

/// A retest scalp alert that is being tracked to outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetestScalpTrade {
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
    /// TP1 already filled; the remaining half is protected at break-even.
    #[serde(default)]
    pub tp1_hit: bool,
    #[serde(default)]
    pub be_active: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RetestScalpOutcome {
    pub outcome: &'static str,
    pub r_multiple: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RetestScalpEvaluation {
    /// TP1 was filled on this candle (state changed).
    pub tp1_just_hit: bool,
    /// Final outcome for the whole plan, if reached on this candle.
    pub outcome: Option<RetestScalpOutcome>,
}

impl RetestScalpTrade {
    pub fn from_watch(
        watch: &RetestWatch,
        direction: String,
        levels: RetestScalpLevels,
        entry_candle_timestamp: i64,
    ) -> Option<Self> {
        if !levels.risk.is_finite() || levels.risk <= 0.0 {
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
            risk: levels.risk,
            created_at: Utc::now(),
            entry_candle_timestamp,
            tp1_hit: false,
            be_active: false,
        })
    }

    pub fn r_tp1(&self) -> f64 {
        (self.take_profit_1 - self.entry).abs() / self.risk
    }

    pub fn r_tp2(&self) -> f64 {
        (self.take_profit_2 - self.entry).abs() / self.risk
    }

    pub fn is_expired(&self, candle_timestamp: i64) -> bool {
        candle_timestamp.saturating_sub(self.entry_candle_timestamp)
            >= RETEST_SCALP_TIMEOUT_BARS * M15_MS
    }

    pub fn timeout_r(&self, close: f64) -> f64 {
        let close_r = match self.direction.as_str() {
            "SELL" => (self.entry - close) / self.risk,
            "BUY" => (close - self.entry) / self.risk,
            _ => 0.0,
        };
        if self.tp1_hit {
            TP1_WEIGHT * self.r_tp1() + TP2_WEIGHT * close_r
        } else {
            close_r
        }
    }

    /// Evaluate one closed M15 candle. SL is always checked before TP levels
    /// (conservative). TP1 may be filled on the same candle as TP2.
    pub fn evaluate(&mut self, candle: &Candle) -> RetestScalpEvaluation {
        let mut evaluation = RetestScalpEvaluation::default();

        // Break-even stop first once TP1 is filled.
        if self.tp1_hit && self.be_active {
            let be_hit = match self.direction.as_str() {
                "SELL" => candle.high >= self.entry,
                "BUY" => candle.low <= self.entry,
                _ => false,
            };
            if be_hit {
                evaluation.outcome = Some(RetestScalpOutcome {
                    outcome: "TP1_BE",
                    r_multiple: TP1_WEIGHT * self.r_tp1(),
                });
                return evaluation;
            }
        }

        // Original stop before TP1.
        if !self.tp1_hit {
            let sl_hit = match self.direction.as_str() {
                "SELL" => candle.high >= self.stop_loss,
                "BUY" => candle.low <= self.stop_loss,
                _ => false,
            };
            if sl_hit {
                evaluation.outcome = Some(RetestScalpOutcome {
                    outcome: "SL",
                    r_multiple: -1.0,
                });
                return evaluation;
            }
        }

        // TP1 partial.
        if !self.tp1_hit {
            let tp1_hit = match self.direction.as_str() {
                "SELL" => candle.low <= self.take_profit_1,
                "BUY" => candle.high >= self.take_profit_1,
                _ => false,
            };
            if tp1_hit {
                self.tp1_hit = true;
                self.be_active = true;
                evaluation.tp1_just_hit = true;
            }
        }

        // TP2 completes the plan. It can happen on the same candle as TP1.
        if self.tp1_hit {
            let tp2_hit = match self.direction.as_str() {
                "SELL" => candle.low <= self.take_profit_2,
                "BUY" => candle.high >= self.take_profit_2,
                _ => false,
            };
            if tp2_hit {
                evaluation.outcome = Some(RetestScalpOutcome {
                    outcome: "TP2",
                    r_multiple: TP1_WEIGHT * self.r_tp1() + TP2_WEIGHT * self.r_tp2(),
                });
                return evaluation;
            }
        }

        if self.is_expired(candle.timestamp) {
            evaluation.outcome = Some(RetestScalpOutcome {
                outcome: "TIMEOUT",
                r_multiple: self.timeout_r(candle.close),
            });
        }

        evaluation
    }
}

async fn record_watch_event(
    state: &AppState,
    event: &str,
    watch: &RetestWatch,
    candle: &Candle,
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
    entry.source = Some("retest_scalp".to_string());
    entry.reason = Some(reason.to_string());
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

async fn record_alert_event(
    state: &AppState,
    trade: &RetestScalpTrade,
    candle: &Candle,
    lot_size: f64,
    risk_usd: f64,
) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new("retest_scalp_alert");
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
    entry.fill_price = Some(trade.entry);
    entry.source = Some("retest_scalp".to_string());
    entry.reason = Some("retest_confirmed".to_string());
    entry.notes = Some(format!(
        "risk_usd={risk_usd:.2};lot={lot_size:.2};tp1_weight={TP1_WEIGHT:.2};tp2_weight={TP2_WEIGHT:.2}"
    ));
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

async fn record_skip_event(
    state: &AppState,
    watch: &RetestWatch,
    candle: &Candle,
    risk: f64,
    max_sl: f64,
) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new("retest_scalp_skipped");
    entry.setup_id = Some(watch.setup_id.clone());
    entry.scenario_id = Some(watch.scenario.id.clone());
    entry.bias = Some(watch.scenario.bias.clone());
    entry.confidence = Some(watch.scenario.confidence);
    entry.created_at = Some(Utc::now());
    entry.candle_timestamp = Some(candle.timestamp);
    entry.entry = Some(watch.broken_level);
    entry.risk_points = Some(risk);
    entry.close = Some(candle.close);
    entry.source = Some("retest_scalp".to_string());
    entry.reason = Some("sl_too_wide".to_string());
    entry.notes = Some(format!("max_sl_usd={max_sl:.2}"));
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

async fn record_tp1_event(state: &AppState, trade: &RetestScalpTrade, candle: &Candle) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new("retest_scalp_tp1");
    entry.setup_id = Some(trade.setup_id.clone());
    entry.scenario_id = Some(trade.scenario_id.clone());
    entry.bias = Some(trade.bias.clone());
    entry.confidence = Some(trade.confidence);
    entry.created_at = Some(Utc::now());
    entry.candle_timestamp = Some(candle.timestamp);
    entry.direction = Some(trade.direction.clone());
    entry.entry = Some(trade.entry);
    entry.stop_loss = Some(trade.entry);
    entry.take_profit_1 = Some(trade.take_profit_1);
    entry.take_profit_2 = Some(trade.take_profit_2);
    entry.risk_points = Some(trade.risk);
    entry.close = Some(candle.close);
    entry.r_multiple = Some(TP1_WEIGHT * trade.r_tp1());
    entry.bars_open = Some((candle.timestamp - trade.entry_candle_timestamp) / M15_MS);
    entry.source = Some("retest_scalp".to_string());
    entry.reason = Some("tp1_filled_break_even".to_string());
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

async fn record_outcome_event(
    state: &AppState,
    trade: &RetestScalpTrade,
    candle: &Candle,
    outcome: RetestScalpOutcome,
) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new("retest_scalp_outcome");
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
    entry.r_multiple = Some(outcome.r_multiple);
    entry.bars_open = Some((candle.timestamp - trade.entry_candle_timestamp) / M15_MS);
    entry.source = Some("retest_scalp".to_string());
    entry.reason = Some(outcome.outcome.to_string());
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

fn direction_for(watch: &RetestWatch) -> Option<&'static str> {
    match watch.scenario.bias.trim().to_uppercase().as_str() {
        "BEARISH" => Some("SELL"),
        "BULLISH" => Some("BUY"),
        _ => None,
    }
}

pub fn spawn_retest_scalp_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "⚡ [Retest Scalp] Worker đã khởi chạy (chu kỳ {}s, wait {} bars, timeout {} bars).",
            RETEST_SCALP_INTERVAL_SECS, RETEST_SCALP_WAIT_BARS, RETEST_SCALP_TIMEOUT_BARS
        );
        let mut interval =
            tokio::time::interval(StdDuration::from_secs(RETEST_SCALP_INTERVAL_SECS));

        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.is_active || !config.retest_scalp_enabled {
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

            // 1) Manage an open scalp trade first.
            let active = state.active_retest_scalp_trade.read().await.clone();
            if let Some(mut trade) = active {
                if candle.timestamp > trade.entry_candle_timestamp {
                    let evaluation = trade.evaluate(&candle);
                    if evaluation.tp1_just_hit {
                        record_tp1_event(&state, &trade, &candle).await;
                        info!(
                            "⚡ [Retest Scalp] {} TP1 filled · BE active",
                            trade.setup_id
                        );
                    }
                    if let Some(outcome) = evaluation.outcome {
                        record_outcome_event(&state, &trade, &candle, outcome).await;
                        info!(
                            "⚡ [Retest Scalp] {} {} · R {:+.2} · bars {}",
                            trade.setup_id,
                            outcome.outcome,
                            outcome.r_multiple,
                            (candle.timestamp - trade.entry_candle_timestamp) / M15_MS
                        );
                        *state.active_retest_scalp_trade.write().await = None;
                        if let Err(err) = persist_state(&state).await {
                            warn!("⚠️ [Retest Scalp] Không lưu được outcome: {}", err);
                        }
                        continue;
                    }
                    if evaluation.tp1_just_hit {
                        *state.active_retest_scalp_trade.write().await = Some(trade);
                        if let Err(err) = persist_state(&state).await {
                            warn!("⚠️ [Retest Scalp] Không lưu được TP1 state: {}", err);
                        }
                        continue;
                    }
                }
                // One open research trade at a time.
                continue;
            }

            // 2) Manage an armed watch.
            let watch = state.retest_scalp_watch.read().await.clone();
            if let Some(watch) = watch {
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
                        "retest_scalp_watch_closed",
                        &watch,
                        &candle,
                        reason,
                    )
                    .await;
                    *state.retest_scalp_watch.write().await = None;
                    if let Err(err) = persist_state(&state).await {
                        warn!("⚠️ [Retest Scalp] Không lưu được clear watch: {}", err);
                    }
                    continue;
                }
                if let Some(levels) = watch.retest_levels(&candle) {
                    let Some(direction) = direction_for(&watch) else {
                        continue;
                    };
                    let risk = (levels.entry - levels.stop_loss).abs();
                    if risk > config.retest_scalp_max_sl_usd {
                        record_skip_event(
                            &state,
                            &watch,
                            &candle,
                            risk,
                            config.retest_scalp_max_sl_usd,
                        )
                        .await;
                        info!(
                            "⚡ [Retest Scalp] Skip {} · SL {:.2} > max {:.2}",
                            watch.setup_id, risk, config.retest_scalp_max_sl_usd
                        );
                        *state.retest_scalp_watch.write().await = None;
                        if let Err(err) = persist_state(&state).await {
                            warn!("⚠️ [Retest Scalp] Không lưu được skip: {}", err);
                        }
                        continue;
                    }
                    let Some(scalp_levels) = RetestScalpLevels::from_retest(
                        &levels,
                        direction,
                        config.retest_scalp_tp1_usd,
                        config.retest_scalp_tp2_usd,
                    ) else {
                        continue;
                    };
                    let Some(mut trade) = RetestScalpTrade::from_watch(
                        &watch,
                        direction.to_string(),
                        scalp_levels,
                        candle.timestamp,
                    ) else {
                        continue;
                    };
                    // Match the backtest: the retest candle itself may reach TP1/TP2.
                    // SL is beyond its wick + buffer, so it cannot hit SL on this candle.
                    let entry_evaluation = trade.evaluate(&candle);
                    let lot = PositionSizer::calculate_lot_size(
                        config.account_balance,
                        config.retest_scalp_risk_percentage,
                        trade.risk,
                        config.retest_scalp_tp1_usd,
                        config.retest_scalp_tp2_usd,
                        candle.close,
                        config.account_leverage,
                    );
                    let telegram = state.telegram.read().await.clone();
                    if let Err(err) = telegram
                        .send_retest_scalp_alert(
                            &watch.scenario,
                            &trade,
                            candle.close,
                            lot.lot_size,
                            lot.risk_amount_usd,
                            config.retest_scalp_risk_percentage,
                        )
                        .await
                    {
                        warn!("⚠️ [Retest Scalp] Không gửi được Telegram: {}", err);
                    }
                    record_alert_event(&state, &trade, &candle, lot.lot_size, lot.risk_amount_usd)
                        .await;
                    info!(
                        "⚡ [Retest Scalp] {} {} · entry {:.2} · SL {:.2} · TP1 {:.2} · TP2 {:.2} · lot {:.2}",
                        trade.scenario_id,
                        trade.direction,
                        trade.entry,
                        trade.stop_loss,
                        trade.take_profit_1,
                        trade.take_profit_2,
                        lot.lot_size
                    );
                    if entry_evaluation.tp1_just_hit {
                        record_tp1_event(&state, &trade, &candle).await;
                        info!(
                            "⚡ [Retest Scalp] {} TP1 filled on retest candle · BE active",
                            trade.setup_id
                        );
                    }
                    if let Some(outcome) = entry_evaluation.outcome {
                        record_outcome_event(&state, &trade, &candle, outcome).await;
                        info!(
                            "⚡ [Retest Scalp] {} {} on retest candle · R {:+.2}",
                            trade.setup_id, outcome.outcome, outcome.r_multiple
                        );
                    } else {
                        *state.active_retest_scalp_trade.write().await = Some(trade);
                    }
                    *state.retest_scalp_watch.write().await = None;
                    if let Err(err) = persist_state(&state).await {
                        warn!("⚠️ [Retest Scalp] Không lưu được trade: {}", err);
                    }
                    continue;
                }
                continue;
            }

            // 3) Arm one watch when a valid PRIMARY triggers. Do not arm while
            // the legacy retest-shadow profile still has an open watch/trade.
            let legacy_busy = state.retest_watch.read().await.is_some()
                || state.active_retest_trade.read().await.is_some();
            if legacy_busy {
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
            if !primary.valid
                || primary.confidence < MIN_CONFIDENCE
                || primary.macro_alignment.eq_ignore_ascii_case("COUNTER")
            {
                continue;
            }
            let Some(side) = direction_for_scenario(primary) else {
                continue;
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
                setup_id: format!("SCALP-{}-{}", candle.timestamp, primary.id),
                scenario: primary.clone(),
                broken_level,
                original_stop_loss,
                trigger_candle_timestamp: candle.timestamp,
                created_at: Utc::now(),
            };
            *state.retest_scalp_watch.write().await = Some(watch.clone());
            record_watch_event(
                &state,
                "retest_scalp_watch_created",
                &watch,
                &candle,
                "trigger_confirmed_wait_retest",
            )
            .await;
            if let Err(err) = persist_state(&state).await {
                warn!("⚠️ [Retest Scalp] Không lưu được watch: {}", err);
            }
            info!(
                "⚡ [Retest Scalp] Armed {} {} · broken {:.2} · wait {} bars",
                watch.scenario.id,
                watch.scenario.trigger_direction,
                broken_level,
                RETEST_SCALP_WAIT_BARS
            );
        }
    });
}

fn direction_for_scenario(scenario: &Scenario) -> Option<&'static str> {
    match scenario.bias.trim().to_uppercase().as_str() {
        "BULLISH" => Some("BUY"),
        "BEARISH" => Some("SELL"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use chrono::TimeZone;

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
            setup_id: "SCALP-TEST".to_string(),
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

    fn trade(direction: &str) -> RetestScalpTrade {
        let (entry, sl, tp1, tp2) = if direction == "SELL" {
            (3800.0, 3810.0, 3780.0, 3760.0)
        } else {
            (3800.0, 3790.0, 3820.0, 3840.0)
        };
        RetestScalpTrade {
            setup_id: "SCALP-TEST".to_string(),
            scenario_id: "bearish_breakdown".to_string(),
            bias: if direction == "SELL" {
                "BEARISH".to_string()
            } else {
                "BULLISH".to_string()
            },
            confidence: 70,
            direction: direction.to_string(),
            entry,
            stop_loss: sl,
            take_profit_1: tp1,
            take_profit_2: tp2,
            risk: 10.0,
            created_at: Utc.with_ymd_and_hms(2026, 11, 18, 0, 0, 0).unwrap(),
            entry_candle_timestamp: 0,
            tp1_hit: false,
            be_active: false,
        }
    }

    #[test]
    fn scalp_levels_use_absolute_tp_distances() {
        let levels = RetestLevels {
            entry: 3800.0,
            stop_loss: 3810.0,
            take_profit_1: 3780.0,
            take_profit_2: 3770.0,
            risk_reward: 2.0,
        };
        let scalp =
            RetestScalpLevels::from_retest(&levels, "SELL", 20.0, 40.0).expect("scalp levels");
        assert_eq!(scalp.take_profit_1, 3780.0);
        assert_eq!(scalp.take_profit_2, 3760.0);
        assert_eq!(scalp.risk, 10.0);
    }

    #[test]
    fn scalp_from_watch_rejects_zero_risk() {
        let watch = watch("BEARISH", 3800.0, 3800.0);
        let levels = RetestScalpLevels {
            entry: 3800.0,
            stop_loss: 3800.0,
            take_profit_1: 3780.0,
            take_profit_2: 3760.0,
            risk: 0.0,
        };
        assert!(RetestScalpTrade::from_watch(&watch, "SELL".to_string(), levels, 0).is_none());
    }

    #[test]
    fn sell_trade_hits_sl_before_tp() {
        let mut trade = trade("SELL");
        let evaluation = trade.evaluate(&candle(3800.0, 3811.0, 3769.0, 3775.0));
        let outcome = evaluation.outcome.expect("outcome");
        assert_eq!(outcome.outcome, "SL");
        assert_eq!(outcome.r_multiple, -1.0);
        assert!(!evaluation.tp1_just_hit);
    }

    #[test]
    fn sell_trade_tp1_then_break_even() {
        let mut trade = trade("SELL");
        let tp1 = trade.evaluate(&candle(3800.0, 3801.0, 3779.0, 3781.0));
        assert!(tp1.tp1_just_hit);
        assert!(tp1.outcome.is_none());
        assert!(trade.tp1_hit);
        assert!(trade.be_active);

        let be = trade.evaluate(&candle(3781.0, 3800.5, 3780.0, 3799.0));
        let outcome = be.outcome.expect("be outcome");
        assert_eq!(outcome.outcome, "TP1_BE");
        assert!((outcome.r_multiple - 1.0).abs() < 1e-9);
    }

    #[test]
    fn sell_trade_tp1_and_tp2_same_candle() {
        let mut trade = trade("SELL");
        let evaluation = trade.evaluate(&candle(3800.0, 3801.0, 3759.0, 3765.0));
        assert!(evaluation.tp1_just_hit);
        let outcome = evaluation.outcome.expect("tp2 outcome");
        assert_eq!(outcome.outcome, "TP2");
        assert!((outcome.r_multiple - 3.0).abs() < 1e-9);
    }

    #[test]
    fn buy_trade_hits_tp2() {
        let mut trade = trade("BUY");
        let evaluation = trade.evaluate(&candle(3800.0, 3841.0, 3799.0, 3835.0));
        let outcome = evaluation.outcome.expect("tp2 outcome");
        assert_eq!(outcome.outcome, "TP2");
        assert!((outcome.r_multiple - 3.0).abs() < 1e-9);
    }

    #[test]
    fn timeout_uses_close_and_tp1_partial() {
        let mut trade = trade("SELL");
        trade.tp1_hit = true;
        trade.be_active = true;
        let mut c = candle(3780.0, 3781.0, 3770.0, 3775.0);
        c.timestamp = RETEST_SCALP_TIMEOUT_BARS * M15_MS;
        let evaluation = trade.evaluate(&c);
        let outcome = evaluation.outcome.expect("timeout outcome");
        assert_eq!(outcome.outcome, "TIMEOUT");
        // 0.5 * 2R + 0.5 * 2.5R = 2.25R
        assert!((outcome.r_multiple - 2.25).abs() < 1e-9);
    }

    #[test]
    fn timeout_before_tp1_uses_full_close_r() {
        let mut trade = trade("SELL");
        let mut c = candle(3790.0, 3791.0, 3785.0, 3790.0);
        c.timestamp = RETEST_SCALP_TIMEOUT_BARS * M15_MS;
        let evaluation = trade.evaluate(&c);
        let outcome = evaluation.outcome.expect("timeout outcome");
        assert_eq!(outcome.outcome, "TIMEOUT");
        assert!((outcome.r_multiple - 1.0).abs() < 1e-9);
    }
}
