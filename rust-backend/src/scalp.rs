//! Scalp OB retest research worker.
//!
//! Alert-only profile for the separate scalp service. After a valid PRIMARY
//! trigger, wait up to 8 M15 bars for a retest of the broken level, require the
//! pre-registered OB distance <= 1.0 ATR, then send a research alert to the
//! scalp channel with TP1 1R / TP2 2R. It never inserts a signal, consumes
//! quota, calls AI or auto-trades.

use crate::action_alerts::candle_is_closed_and_fresh;
use crate::api::{current_scenarios, AppState};
use crate::market_context::htf_allows_signal;
use crate::models::Candle;
use crate::retest_scalp::{RetestScalpLevels, RetestScalpOutcome, RetestScalpTrade};
use crate::retest_shadow::RetestWatch;
use crate::risk_manager::PositionSizer;
use crate::scalp_config::ScalpFilterStore;
use crate::scenarios::{scenario_triggered, Scenario};
use crate::storage::PersistentStore;
use crate::trigger_journal::{record, TriggerEvent};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration as StdDuration;
use tracing::{info, warn};

const INTERVAL_SECS: u64 = 30;
const WAIT_BARS: i64 = 8;
const M15_MS: i64 = 15 * 60 * 1000;
const OB_MAX_ATR: f64 = 1.0;
const TP1_WEIGHT: f64 = 0.5;
const TP2_WEIGHT: f64 = 0.5;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ScalpObState {
    #[serde(default)]
    watch: Option<RetestWatch>,
    #[serde(default)]
    trade: Option<RetestScalpTrade>,
    #[serde(default)]
    alerts: Vec<DateTime<Utc>>,
    #[serde(default)]
    last_alert_at: Option<DateTime<Utc>>,
}

struct ScalpObStore {
    path: PathBuf,
}

impl ScalpObStore {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn load(&self) -> ScalpObState {
        let Ok(content) = fs::read_to_string(&self.path) else {
            return ScalpObState::default();
        };
        match serde_json::from_str::<ScalpObState>(&content) {
            Ok(state) => state,
            Err(err) => {
                warn!(
                    "⚠️ [Scalp OB] State {} không hợp lệ ({}). Dùng state mới.",
                    self.path.display(),
                    err
                );
                ScalpObState::default()
            }
        }
    }

    fn save(&self, state: &ScalpObState) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let temp = self.path.with_extension("json.tmp");
        let payload = serde_json::to_string_pretty(state).map_err(|err| err.to_string())?;
        fs::write(&temp, payload).map_err(|err| err.to_string())?;
        fs::rename(&temp, &self.path).map_err(|err| err.to_string())
    }
}

fn env_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn atr14_at(candles: &[Candle], index: usize) -> Option<f64> {
    if index < 14 || index >= candles.len() {
        return None;
    }
    let mut sum = 0.0;
    for i in (index - 13)..=index {
        let candle = &candles[i];
        let previous = if i > 0 {
            candles[i - 1].close
        } else {
            candle.close
        };
        let tr = (candle.high - candle.low)
            .max((candle.high - previous).abs())
            .max((candle.low - previous).abs());
        sum += tr;
    }
    let atr = sum / 14.0;
    (atr.is_finite() && atr > 0.0).then_some(atr)
}

/// Pre-registered OB distance: last opposite-color candle body before a
/// displacement >= 1.0 ATR that breaks the prior 10-bar extreme. Distance is
/// in ATR from the retest entry; 0 means entry is at/below the OB for SELL or
/// at/above it for BUY.
pub fn ob_distance_atr(
    candles: &[Candle],
    index: usize,
    direction: &str,
    entry: f64,
) -> Option<f64> {
    if index < 20 || index >= candles.len() {
        return None;
    }
    let atr = atr14_at(candles, index)?;
    let start = index.saturating_sub(50).max(15);
    for j in (start..=index).rev() {
        let candle = &candles[j];
        let previous = &candles[j.saturating_sub(10)..j];
        if previous.len() < 10 {
            continue;
        }
        let range = candle.high - candle.low;
        let displacement = match direction {
            "SELL" => {
                candle.close < candle.open
                    && range >= atr
                    && candle.close
                        < previous
                            .iter()
                            .map(|item| item.low)
                            .fold(f64::INFINITY, f64::min)
            }
            "BUY" => {
                candle.close > candle.open
                    && range >= atr
                    && candle.close
                        > previous
                            .iter()
                            .map(|item| item.high)
                            .fold(f64::NEG_INFINITY, f64::max)
            }
            _ => false,
        };
        if !displacement {
            continue;
        }
        for k in (j.saturating_sub(5)..j).rev() {
            let ob = &candles[k];
            let is_opposite = match direction {
                "SELL" => ob.close > ob.open,
                "BUY" => ob.close < ob.open,
                _ => false,
            };
            if !is_opposite {
                continue;
            }
            let zone_low = ob.open.min(ob.close);
            let zone_high = ob.open.max(ob.close);
            let distance = match direction {
                "SELL" => (entry - zone_high).max(0.0) / atr,
                "BUY" => (zone_low - entry).max(0.0) / atr,
                _ => return None,
            };
            return Some(distance);
        }
        return None;
    }
    None
}

fn direction_for_scenario(scenario: &Scenario) -> Option<&'static str> {
    match scenario.bias.trim().to_uppercase().as_str() {
        "BULLISH" => Some("BUY"),
        "BEARISH" => Some("SELL"),
        _ => None,
    }
}

fn direction_for(watch: &RetestWatch) -> Option<&'static str> {
    direction_for_scenario(&watch.scenario)
}

fn vn_date(value: DateTime<Utc>) -> chrono::NaiveDate {
    (value + Duration::hours(7)).date_naive()
}

fn same_vn_day(left: DateTime<Utc>, right: DateTime<Utc>) -> bool {
    vn_date(left) == vn_date(right)
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
    entry.source = Some("scalp_ob".to_string());
    entry.reason = Some(reason.to_string());
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
    reason: &str,
    ob_distance: Option<f64>,
) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new("scalp_ob_skipped");
    entry.setup_id = Some(watch.setup_id.clone());
    entry.scenario_id = Some(watch.scenario.id.clone());
    entry.bias = Some(watch.scenario.bias.clone());
    entry.confidence = Some(watch.scenario.confidence);
    entry.created_at = Some(Utc::now());
    entry.candle_timestamp = Some(candle.timestamp);
    entry.entry = Some(watch.broken_level);
    entry.close = Some(candle.close);
    entry.source = Some("scalp_ob".to_string());
    entry.reason = Some(reason.to_string());
    entry.notes = ob_distance.map(|value| format!("ob_distance_atr={value:.3}"));
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

#[allow(clippy::too_many_arguments)]
async fn record_alert_event(
    state: &AppState,
    trade: &RetestScalpTrade,
    candle: &Candle,
    lot_size: f64,
    risk_usd: f64,
    ob_distance: f64,
) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new("scalp_ob_alert");
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
    entry.source = Some("scalp_ob".to_string());
    entry.reason = Some("ob_retest_confirmed".to_string());
    entry.notes = Some(format!(
        "ob_distance_atr={ob_distance:.3};risk_usd={risk_usd:.2};lot={lot_size:.2};tp1_weight={TP1_WEIGHT:.2};tp2_weight={TP2_WEIGHT:.2}"
    ));
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

async fn record_tp1_event(state: &AppState, trade: &RetestScalpTrade, candle: &Candle) {
    let config = state.config.read().await.clone();
    let mut entry = TriggerEvent::new("scalp_ob_tp1");
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
    entry.source = Some("scalp_ob".to_string());
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
    let mut entry = TriggerEvent::new("scalp_ob_outcome");
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
    entry.source = Some("scalp_ob".to_string());
    entry.reason = Some(outcome.outcome.to_string());
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &entry,
    );
}

pub fn spawn_scalp_worker(state: AppState) {
    tokio::spawn(async move {
        if !env_enabled("SCALP_OB_ALERTS_ENABLED") {
            info!("🧪 [Scalp OB] Worker disabled (SCALP_OB_ALERTS_ENABLED is not true).");
            return;
        }
        let path = std::env::var("SCALP_OB_STATE_PATH")
            .unwrap_or_else(|_| "data/scalp_ob_state.json".to_string());
        let store = ScalpObStore::new(path);
        let filter_store = ScalpFilterStore::from_env();
        let mut local = store.load();
        info!(
            "🧪 [Scalp OB] Worker started (wait {} bars, OB <= {:.1} ATR, TP 1R/2R, filters config-driven).",
            WAIT_BARS, OB_MAX_ATR
        );
        let mut interval = tokio::time::interval(StdDuration::from_secs(INTERVAL_SECS));

        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            let filters = filter_store.load();
            if !config.is_active {
                continue;
            }
            if PersistentStore::is_market_closed_weekend(Utc::now()) {
                continue;
            }
            let candles = state.candles.read().await.clone();
            let Some(candle) = candles.last().cloned() else {
                continue;
            };
            if !candle_is_closed_and_fresh(
                &candle,
                Utc::now().timestamp_millis(),
                config.max_candle_age_secs,
            ) {
                continue;
            }

            // 1) Manage an open research trade.
            if let Some(mut trade) = local.trade.clone() {
                if candle.timestamp > trade.entry_candle_timestamp {
                    let evaluation = trade.evaluate(&candle);
                    if evaluation.tp1_just_hit {
                        record_tp1_event(&state, &trade, &candle).await;
                        info!("🧪 [Scalp OB] {} TP1 filled · BE active", trade.setup_id);
                    }
                    if let Some(outcome) = evaluation.outcome {
                        record_outcome_event(&state, &trade, &candle, outcome).await;
                        info!(
                            "🧪 [Scalp OB] {} {} · R {:+.2} · bars {}",
                            trade.setup_id,
                            outcome.outcome,
                            outcome.r_multiple,
                            (candle.timestamp - trade.entry_candle_timestamp) / M15_MS
                        );
                        local.trade = None;
                        if let Err(err) = store.save(&local) {
                            warn!("⚠️ [Scalp OB] Không lưu được outcome: {}", err);
                        }
                        continue;
                    }
                    if evaluation.tp1_just_hit {
                        local.trade = Some(trade);
                        if let Err(err) = store.save(&local) {
                            warn!("⚠️ [Scalp OB] Không lưu được TP1 state: {}", err);
                        }
                        continue;
                    }
                }
                continue;
            }

            // 2) Manage an armed watch.
            if let Some(watch) = local.watch.clone() {
                if candle.timestamp <= watch.trigger_candle_timestamp {
                    continue;
                }
                if watch.is_expired(candle.timestamp) || watch.is_invalidated(&candle) {
                    let reason = if watch.is_expired(candle.timestamp) {
                        "expired"
                    } else {
                        "invalidated"
                    };
                    record_watch_event(&state, "scalp_ob_watch_closed", &watch, &candle, reason)
                        .await;
                    local.watch = None;
                    if let Err(err) = store.save(&local) {
                        warn!("⚠️ [Scalp OB] Không lưu được clear watch: {}", err);
                    }
                    continue;
                }
                if let Some(levels) = watch.retest_levels(&candle) {
                    let Some(direction) = direction_for(&watch) else {
                        continue;
                    };
                    let index = candles.len() - 1;
                    let ob_distance = ob_distance_atr(&candles, index, direction, levels.entry);
                    let Some(ob_distance) = ob_distance else {
                        record_skip_event(&state, &watch, &candle, "ob_missing", None).await;
                        local.watch = None;
                        if let Err(err) = store.save(&local) {
                            warn!("⚠️ [Scalp OB] Không lưu được skip: {}", err);
                        }
                        continue;
                    };
                    if ob_distance > OB_MAX_ATR {
                        record_skip_event(&state, &watch, &candle, "ob_too_far", Some(ob_distance))
                            .await;
                        local.watch = None;
                        if let Err(err) = store.save(&local) {
                            warn!("⚠️ [Scalp OB] Không lưu được skip: {}", err);
                        }
                        continue;
                    }
                    let risk = (levels.entry - levels.stop_loss).abs();
                    if !risk.is_finite() || risk <= 0.0 {
                        continue;
                    }
                    let Some(scalp_levels) =
                        RetestScalpLevels::from_retest(&levels, direction, risk, risk * 2.0)
                    else {
                        continue;
                    };
                    let now = Utc::now();
                    let alerts_today = local
                        .alerts
                        .iter()
                        .filter(|at| same_vn_day(**at, now))
                        .count();
                    if alerts_today >= filters.max_alerts_per_day {
                        record_skip_event(&state, &watch, &candle, "daily_cap", Some(ob_distance))
                            .await;
                        local.watch = None;
                        if let Err(err) = store.save(&local) {
                            warn!("⚠️ [Scalp OB] Không lưu được cap: {}", err);
                        }
                        continue;
                    }
                    if let Some(last) = local.last_alert_at {
                        if now - last < Duration::minutes(filters.cooldown_minutes) {
                            record_skip_event(
                                &state,
                                &watch,
                                &candle,
                                "cooldown",
                                Some(ob_distance),
                            )
                            .await;
                            local.watch = None;
                            if let Err(err) = store.save(&local) {
                                warn!("⚠️ [Scalp OB] Không lưu được cooldown: {}", err);
                            }
                            continue;
                        }
                    }
                    let Some(mut trade) = RetestScalpTrade::from_watch(
                        &watch,
                        direction.to_string(),
                        scalp_levels,
                        candle.timestamp,
                    ) else {
                        continue;
                    };
                    let entry_evaluation = trade.evaluate(&candle);
                    let lot = PositionSizer::calculate_lot_size(
                        config.account_balance,
                        config.risk_percentage,
                        trade.risk,
                        trade.r_tp1() * trade.risk,
                        trade.r_tp2() * trade.risk,
                        candle.close,
                        config.account_leverage,
                    );
                    let telegram = state.telegram.read().await.clone();
                    if let Err(err) = telegram
                        .send_scalp_ob_alert(
                            &watch.scenario,
                            &trade,
                            candle.close,
                            lot.lot_size,
                            lot.risk_amount_usd,
                            config.risk_percentage,
                            ob_distance,
                        )
                        .await
                    {
                        warn!("⚠️ [Scalp OB] Không gửi được Telegram: {}", err);
                    }
                    record_alert_event(
                        &state,
                        &trade,
                        &candle,
                        lot.lot_size,
                        lot.risk_amount_usd,
                        ob_distance,
                    )
                    .await;
                    info!(
                        "🧪 [Scalp OB] {} {} · entry {:.2} · SL {:.2} · TP1 {:.2} · TP2 {:.2} · OB {:.2}×ATR · lot {:.2}",
                        trade.scenario_id,
                        trade.direction,
                        trade.entry,
                        trade.stop_loss,
                        trade.take_profit_1,
                        trade.take_profit_2,
                        ob_distance,
                        lot.lot_size
                    );
                    local.alerts.push(now);
                    local.last_alert_at = Some(now);
                    local.alerts.retain(|at| now - *at <= Duration::days(7));
                    if entry_evaluation.tp1_just_hit {
                        record_tp1_event(&state, &trade, &candle).await;
                    }
                    if let Some(outcome) = entry_evaluation.outcome {
                        record_outcome_event(&state, &trade, &candle, outcome).await;
                    } else {
                        local.trade = Some(trade);
                    }
                    local.watch = None;
                    if let Err(err) = store.save(&local) {
                        warn!("⚠️ [Scalp OB] Không lưu được trade: {}", err);
                    }
                    continue;
                }
                continue;
            }

            // 3) Arm one watch when a valid PRIMARY triggers.
            let scenarios = current_scenarios(&state).await;
            let Some(primary) = scenarios
                .scenarios
                .iter()
                .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"))
            else {
                continue;
            };
            if !primary.valid
                || primary.confidence < filters.min_confidence
                || primary.macro_alignment.eq_ignore_ascii_case("COUNTER")
            {
                continue;
            }
            let Some(side) = direction_for_scenario(primary) else {
                continue;
            };
            if let Some(context) = state.market_context.read().await.clone() {
                let htf_h4 = filters.htf_filter_enabled && config.htf_h4_filter_enabled;
                let htf_d1 = filters.htf_filter_enabled && config.htf_d1_filter_enabled;
                let (allowed, _) = htf_allows_signal(side, &context, htf_h4, htf_d1);
                if !allowed {
                    continue;
                }
                if filters.rsi_h1_guard_enabled
                    && config.min_rsi_h1_for_buy > 0.0
                    && side == "BUY"
                    && context
                        .rsi_h1
                        .as_ref()
                        .and_then(|rsi| rsi.value)
                        .is_some_and(|value| value < config.min_rsi_h1_for_buy)
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
                setup_id: format!("SCALPOB-{}-{}", candle.timestamp, primary.id),
                scenario: primary.clone(),
                broken_level,
                original_stop_loss,
                trigger_candle_timestamp: candle.timestamp,
                created_at: Utc::now(),
            };
            local.watch = Some(watch.clone());
            record_watch_event(
                &state,
                "scalp_ob_watch_created",
                &watch,
                &candle,
                "trigger_confirmed_wait_retest",
            )
            .await;
            if let Err(err) = store.save(&local) {
                warn!("⚠️ [Scalp OB] Không lưu được watch: {}", err);
            }
            info!(
                "🧪 [Scalp OB] Armed {} {} · broken {:.2} · wait {} bars",
                watch.scenario.id, watch.scenario.trigger_direction, broken_level, WAIT_BARS
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;

    fn candle(timestamp: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume: 1000.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    #[test]
    fn atr14_is_positive() {
        let candles: Vec<Candle> = (0..20)
            .map(|i| candle(i * M15_MS, 100.0, 101.0, 99.0, 100.0))
            .collect();
        let atr = atr14_at(&candles, 19).expect("atr");
        assert!(atr > 0.0);
    }

    #[test]
    fn ob_distance_detects_displacement_and_body_zone() {
        let mut candles: Vec<Candle> = (0..20)
            .map(|i| candle(i * M15_MS, 100.0, 101.0, 99.0, 100.0))
            .collect();
        // Bullish OB body 100.0..100.5.
        candles.push(candle(20 * M15_MS, 100.0, 101.0, 99.0, 100.5));
        // Bearish displacement below the prior 10-bar low.
        candles.push(candle(21 * M15_MS, 100.5, 101.0, 95.0, 95.5));
        let distance = ob_distance_atr(&candles, 21, "SELL", 102.0).expect("ob");
        assert!(distance > 0.0);
    }
}
