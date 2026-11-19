//! Append-only trigger journal for measuring raw trigger efficacy.
//!
//! This is deliberately separate from `bot_state.json`: it is an analysis log,
//! not live trade state. Every setup, trigger, AI review and hypothetical
//! outcome is appended as one JSON line so the strategy can be evaluated even
//! when confirmation/AI/risk blocked the signal.
//!
//! Schema v1:
//! - `scenario_observed`: one valid PRIMARY observation per M15 close.
//! - `setup_created`: a SETUP WATCHING alert was sent; levels are frozen.
//! - `setup_closed`: the watched setup expired/invalidated before triggering.
//! - `triggered`: the M15 close crossed the frozen trigger (confirmation may
//!   still pass or fail).
//! - `ai_review`: AI action/confidence/model for that trigger, including
//!   provider-unavailable cases.
//! - `outcome`: hypothetical TP1/SL/TIMEOUT result plus MFE/MAE checkpoints.

use crate::models::Candle;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use tracing::warn;

pub const TRIGGER_JOURNAL_SCHEMA: u8 = 1;
/// Forward window for the hypothetical trigger outcome (96 M15 bars = 24h),
/// matching the backtest simulator for shadow/A-B comparability.
pub const TRIGGER_OUTCOME_MAX_BARS: usize = 96;

/// One journal line. Optional fields are omitted from JSON when `None` to keep
/// the file compact; `#[serde(default)]` keeps old/new lines compatible.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TriggerEvent {
    pub schema: u8,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candle_timestamp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_loss: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take_profit_1: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take_profit_2: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_points: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_total: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_structure: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_trigger: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_macro: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bars_open: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub macro_alignment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h4_atr: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch_distance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_price: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_slippage: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_pass: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_type: Option<String>,
    /// Decision identity/versioning for replay and governance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_hash: Option<String>,
    /// Gate that blocked/allowed the decision, with a specific sub-reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate_subreason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_to_gate_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift_at_gate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_ema_atr: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsi_h1: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsi_m15: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sl_distance_atr: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atr_percentile: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wick_body_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone_touch_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone_width_atr: Option<f64>,
    /// Shadow research: blocked decision tracked with the same exit simulator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow_outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_blocked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_view: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r_multiple: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bars_to_outcome: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfe_1_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mae_1_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfe_4_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mae_4_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfe_8_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mae_8_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mfe_24_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mae_24_r: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl TriggerEvent {
    pub fn new(event: &str) -> Self {
        Self {
            schema: TRIGGER_JOURNAL_SCHEMA,
            event: event.to_string(),
            created_at: Some(Utc::now()),
            ..Self::default()
        }
    }
}

/// Append-only JSONL store.
#[derive(Debug, Clone)]
pub struct TriggerJournal {
    path: PathBuf,
}

impl TriggerJournal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &TriggerEvent) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|err| format!("Không tạo được thư mục journal: {err}"))?;
            }
        }
        let line = serde_json::to_string(event)
            .map_err(|err| format!("Không serialize được trigger journal: {err}"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| {
                format!(
                    "Không mở được trigger journal {}: {err}",
                    self.path.display()
                )
            })?;
        writeln!(file, "{line}").map_err(|err| format!("Không ghi được trigger journal: {err}"))?;
        Ok(())
    }

    /// Load journal lines. `limit` keeps the newest N events.
    pub fn load(&self, limit: Option<usize>) -> Vec<TriggerEvent> {
        if !self.path.exists() {
            return Vec::new();
        }
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(err) => {
                warn!(
                    "⚠️ Không đọc được trigger journal {}: {}",
                    self.path.display(),
                    err
                );
                return Vec::new();
            }
        };
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    warn!("⚠️ Trigger journal line lỗi: {}", err);
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<TriggerEvent>(&line) {
                Ok(event) => events.push(event),
                Err(err) => warn!("⚠️ Trigger journal JSON lỗi (bỏ qua line): {}", err),
            }
        }
        if let Some(limit) = limit {
            if events.len() > limit {
                events = events.split_off(events.len() - limit);
            }
        }
        events
    }

    pub fn summary(&self) -> TriggerSummary {
        summarize(&self.load(None))
    }

    /// Trigger events that have no outcome yet.
    pub fn open_triggers(&self) -> Vec<OpenTrigger> {
        open_triggers_from_events(&self.load(None))
    }

    /// True when a `triggered` event already exists for this setup id.
    /// Prevents duplicate journal lines when the scanner retries a rejected
    /// trigger on the same M15 candle.
    pub fn has_triggered(&self, setup_id: &str) -> bool {
        self.load(None)
            .iter()
            .any(|event| event.event == "triggered" && event.setup_id.as_deref() == Some(setup_id))
    }
}

/// A triggered setup that still needs a hypothetical outcome.
#[derive(Debug, Clone, Default)]
pub struct OpenTrigger {
    pub setup_id: String,
    pub signal_id: Option<String>,
    /// BUY | SELL
    pub direction: String,
    pub entry: f64,
    pub stop_loss: f64,
    pub take_profit_1: f64,
    pub trigger_candle_timestamp: i64,
    pub triggered_at: DateTime<Utc>,
    pub decision_id: Option<String>,
    pub strategy_version: Option<String>,
    pub config_hash: Option<String>,
    pub snapshot_hash: Option<String>,
    pub gate: Option<String>,
    pub gate_subreason: Option<String>,
    pub score: Option<u8>,
    pub trigger_to_gate_ms: Option<i64>,
    pub drift_at_gate: Option<f64>,
    pub distance_ema_atr: Option<f64>,
    /// True when the trigger was blocked by confirmation/AI/risk and is tracked
    /// as research rather than a live signal.
    pub shadow: bool,
}

impl OpenTrigger {
    fn from_event(event: &TriggerEvent) -> Option<Self> {
        let setup_id = event.setup_id.clone()?;
        let direction = match event
            .bias
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_uppercase()
            .as_str()
        {
            "BEARISH" => "SELL",
            "BULLISH" => "BUY",
            _ => return None,
        };
        Some(Self {
            setup_id,
            signal_id: event.signal_id.clone(),
            direction: direction.to_string(),
            entry: event.entry?,
            stop_loss: event.stop_loss?,
            take_profit_1: event.take_profit_1?,
            trigger_candle_timestamp: event.candle_timestamp?,
            triggered_at: event.created_at.unwrap_or_else(Utc::now),
            decision_id: event.decision_id.clone(),
            strategy_version: event.strategy_version.clone(),
            config_hash: event.config_hash.clone(),
            snapshot_hash: event.snapshot_hash.clone(),
            gate: event.gate.clone(),
            gate_subreason: event.gate_subreason.clone(),
            score: event.score,
            trigger_to_gate_ms: event.trigger_to_gate_ms,
            drift_at_gate: event.drift_at_gate,
            distance_ema_atr: event.distance_ema_atr,
            shadow: event.confirmation_pass != Some(true),
        })
    }
}

pub fn open_triggers_from_events(events: &[TriggerEvent]) -> Vec<OpenTrigger> {
    let mut completed = HashSet::new();
    for event in events {
        if event.event == "outcome" {
            if let Some(setup_id) = event.setup_id.as_ref() {
                completed.insert(setup_id.clone());
            }
        }
    }
    let ai_blocked: HashSet<String> = events
        .iter()
        .filter(|event| event.event == "ai_review" && event.ai_blocked == Some(true))
        .filter_map(|event| event.setup_id.clone())
        .collect();
    let mut seen = HashSet::new();
    let mut open = Vec::new();
    for event in events {
        if event.event != "triggered" {
            continue;
        }
        let Some(setup_id) = event.setup_id.as_ref() else {
            continue;
        };
        if completed.contains(setup_id) || !seen.insert(setup_id.clone()) {
            continue;
        }
        if let Some(mut trigger) = OpenTrigger::from_event(event) {
            if ai_blocked.contains(setup_id) {
                trigger.shadow = true;
            }
            open.push(trigger);
        }
    }
    open
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TriggerOutcome {
    pub outcome: String,
    pub r_multiple: f64,
    pub bars_to_outcome: usize,
    pub mfe_1_r: Option<f64>,
    pub mae_1_r: Option<f64>,
    pub mfe_4_r: Option<f64>,
    pub mae_4_r: Option<f64>,
    pub mfe_8_r: Option<f64>,
    pub mae_8_r: Option<f64>,
    pub mfe_24_r: Option<f64>,
    pub mae_24_r: Option<f64>,
}

#[derive(Debug, Clone)]
pub enum OutcomeEvaluation {
    Pending,
    Complete(TriggerOutcome),
    DataGap,
}

/// Evaluate the hypothetical outcome of a trigger from the candles *after* the
/// confirming candle. Both TP and SL in the same candle count as SL first
/// (conservative). The forward window is 24 M15 bars.
pub fn evaluate_outcome(
    trigger: &OpenTrigger,
    candles: &[Candle],
    max_bars: usize,
) -> OutcomeEvaluation {
    let Some(start) = candles
        .iter()
        .position(|candle| candle.timestamp == trigger.trigger_candle_timestamp)
    else {
        return OutcomeEvaluation::DataGap;
    };
    let forward = &candles[start + 1..];
    if forward.is_empty() {
        return OutcomeEvaluation::Pending;
    }
    let risk = (trigger.entry - trigger.stop_loss).abs();
    if !risk.is_finite() || risk <= 0.0 {
        return OutcomeEvaluation::DataGap;
    }
    let is_sell = trigger.direction.eq_ignore_ascii_case("SELL");
    let max_bars = max_bars.max(1);
    let checkpoints = [1usize, 4, 8, 24];
    let mut mfe = 0.0_f64;
    let mut mae = 0.0_f64;
    let mut mfe_at: [Option<f64>; 4] = [None; 4];
    let mut mae_at: [Option<f64>; 4] = [None; 4];
    let mut result: Option<(String, f64, usize)> = None;

    for (index, candle) in forward.iter().enumerate() {
        let bars = index + 1;
        let favorable = if is_sell {
            trigger.entry - candle.low
        } else {
            candle.high - trigger.entry
        };
        let adverse = if is_sell {
            candle.high - trigger.entry
        } else {
            trigger.entry - candle.low
        };
        mfe = mfe.max(favorable);
        mae = mae.max(adverse);
        for (slot, checkpoint) in checkpoints.iter().enumerate() {
            if bars == *checkpoint {
                mfe_at[slot] = Some(mfe / risk);
                mae_at[slot] = Some(mae / risk);
            }
        }

        let sl_hit = if is_sell {
            candle.high >= trigger.stop_loss
        } else {
            candle.low <= trigger.stop_loss
        };
        let tp_hit = if is_sell {
            candle.low <= trigger.take_profit_1
        } else {
            candle.high >= trigger.take_profit_1
        };

        if sl_hit {
            result = Some(("SL".to_string(), -1.0, bars));
            break;
        }
        if tp_hit {
            let reward = (trigger.take_profit_1 - trigger.entry).abs();
            result = Some(("TP1".to_string(), reward / risk, bars));
            break;
        }
        if bars >= max_bars {
            let close_r = if is_sell {
                (trigger.entry - candle.close) / risk
            } else {
                (candle.close - trigger.entry) / risk
            };
            result = Some(("TIMEOUT".to_string(), close_r, bars));
            break;
        }
    }

    let Some((outcome, r_multiple, bars_to_outcome)) = result else {
        return OutcomeEvaluation::Pending;
    };
    OutcomeEvaluation::Complete(TriggerOutcome {
        outcome,
        r_multiple,
        bars_to_outcome,
        mfe_1_r: mfe_at[0],
        mae_1_r: mae_at[0],
        mfe_4_r: mfe_at[1],
        mae_4_r: mae_at[1],
        mfe_8_r: mfe_at[2],
        mae_8_r: mae_at[2],
        mfe_24_r: mfe_at[3],
        mae_24_r: mae_at[3],
    })
}

/// Append best-effort; journal failures must never affect signals.
pub fn record(journal: &TriggerJournal, enabled: bool, event: &TriggerEvent) {
    if !enabled {
        return;
    }
    if let Err(err) = journal.append(event) {
        warn!("⚠️ Trigger journal append lỗi: {}", err);
    }
}

/// Coarse regime label from D1/H4/H1 bias for bucket analysis.
pub fn regime_label(d1: &str, h4: &str, h1: &str) -> String {
    let d1 = d1.trim().to_uppercase();
    let h4 = h4.trim().to_uppercase();
    let h1 = h1.trim().to_uppercase();
    if d1 == h4 && h4 == h1 && matches!(d1.as_str(), "BULLISH" | "BEARISH") {
        if d1 == "BULLISH" {
            "TREND_BULL".to_string()
        } else {
            "TREND_BEAR".to_string()
        }
    } else if d1 != h4 {
        "RANGE".to_string()
    } else {
        "MIXED".to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TriggerSummary {
    pub schema: u8,
    pub watching_setups: usize,
    pub watching_triggered: usize,
    pub scanner_triggers: usize,
    pub watching_trigger_rate: f64,
    pub outcomes: usize,
    pub wins: usize,
    pub losses: usize,
    pub timeouts: usize,
    pub data_gaps: usize,
    pub win_rate: f64,
    pub avg_r: f64,
    pub expectancy: f64,
    pub profit_factor: Option<f64>,
    pub max_drawdown_r: f64,
    pub by_scenario: Vec<BucketSummary>,
    pub by_regime: Vec<BucketSummary>,
    pub by_session: Vec<BucketSummary>,
    pub by_ai_confidence: Vec<BucketSummary>,
    pub by_confirmation: Vec<BucketSummary>,
    pub by_confirmation_type: Vec<BucketSummary>,
    pub by_score: Vec<BucketSummary>,
    pub strategist_views: usize,
    pub strategist_models: Vec<CountBucket>,
    pub strategist_primaries: Vec<CountBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CountBucket {
    pub key: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BucketSummary {
    pub key: String,
    pub outcomes: usize,
    pub wins: usize,
    pub losses: usize,
    pub timeouts: usize,
    pub win_rate: f64,
    pub avg_r: f64,
    pub expectancy: f64,
    pub profit_factor: Option<f64>,
}

pub fn summarize(events: &[TriggerEvent]) -> TriggerSummary {
    let mut created = HashSet::new();
    let mut triggered: HashMap<String, &TriggerEvent> = HashMap::new();
    let mut setup_meta: HashMap<String, &TriggerEvent> = HashMap::new();
    let mut ai_reviews: HashMap<String, &TriggerEvent> = HashMap::new();
    let mut outcomes: Vec<&TriggerEvent> = Vec::new();
    let mut strategist_views = 0usize;
    let mut strategist_models: HashMap<String, usize> = HashMap::new();
    let mut strategist_primaries: HashMap<String, usize> = HashMap::new();

    for event in events {
        if event.event == "strategist_view" {
            strategist_views += 1;
            let model = event
                .ai_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            *strategist_models.entry(model).or_default() += 1;
            let primary = event
                .scenario_id
                .clone()
                .unwrap_or_else(|| "none".to_string());
            *strategist_primaries.entry(primary).or_default() += 1;
            continue;
        }
        let Some(setup_id) = event.setup_id.as_ref() else {
            continue;
        };
        match event.event.as_str() {
            "setup_created" => {
                created.insert(setup_id.clone());
                setup_meta.entry(setup_id.clone()).or_insert(event);
            }
            "triggered" => {
                triggered.entry(setup_id.clone()).or_insert(event);
                setup_meta.entry(setup_id.clone()).or_insert(event);
            }
            "ai_review" => {
                ai_reviews.insert(setup_id.clone(), event);
            }
            "outcome" => outcomes.push(event),
            _ => {}
        }
    }

    let watching_setups = created.len();
    let watching_triggered = triggered
        .keys()
        .filter(|setup_id| created.contains(*setup_id))
        .count();
    let scanner_triggers = triggered.len().saturating_sub(watching_triggered);
    let watching_trigger_rate = if watching_setups > 0 {
        watching_triggered as f64 / watching_setups as f64
    } else {
        0.0
    };

    let mut wins = 0;
    let mut losses = 0;
    let mut timeouts = 0;
    let mut data_gaps = 0;
    let mut sum_r = 0.0;
    let mut gross_profit = 0.0;
    let mut gross_loss = 0.0;
    let mut r_values: Vec<(DateTime<Utc>, f64)> = Vec::new();
    let mut scenario_map: HashMap<String, Vec<f64>> = HashMap::new();
    let mut regime_map: HashMap<String, Vec<f64>> = HashMap::new();
    let mut session_map: HashMap<String, Vec<f64>> = HashMap::new();
    let mut ai_map: HashMap<String, Vec<f64>> = HashMap::new();
    let mut confirmation_map: HashMap<String, Vec<f64>> = HashMap::new();
    let mut confirmation_type_map: HashMap<String, Vec<f64>> = HashMap::new();
    let mut score_map: HashMap<String, Vec<f64>> = HashMap::new();

    for outcome in &outcomes {
        let Some(setup_id) = outcome.setup_id.as_ref() else {
            continue;
        };
        let kind = outcome.outcome.as_deref().unwrap_or("UNKNOWN");
        if kind == "DATA_GAP" {
            data_gaps += 1;
            continue;
        }
        let r = outcome.r_multiple.unwrap_or(0.0);
        match kind {
            "TP1" => wins += 1,
            "SL" => losses += 1,
            "TIMEOUT" => timeouts += 1,
            _ => {}
        }
        sum_r += r;
        if r > 0.0 {
            gross_profit += r;
        } else {
            gross_loss += r.abs();
        }
        let at = triggered
            .get(setup_id)
            .and_then(|event| event.created_at)
            .or(outcome.created_at)
            .unwrap_or_else(Utc::now);
        r_values.push((at, r));

        let meta = setup_meta.get(setup_id);
        let trigger_event = triggered.get(setup_id);
        let scenario = meta
            .and_then(|event| event.scenario_id.clone())
            .or_else(|| trigger_event.and_then(|event| event.scenario_id.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        let regime = meta
            .and_then(|event| event.regime.clone())
            .or_else(|| trigger_event.and_then(|event| event.regime.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        let session = meta
            .and_then(|event| event.session.clone())
            .or_else(|| trigger_event.and_then(|event| event.session.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        let confirmation = trigger_event
            .and_then(|event| event.confirmation_pass)
            .map(|pass| if pass { "CONFIRMED" } else { "REJECTED" })
            .unwrap_or("UNKNOWN");
        let confirmation_type = trigger_event
            .and_then(|event| event.confirmation_type.clone())
            .unwrap_or_else(|| "UNKNOWN".to_string());
        let score_total = trigger_event
            .and_then(|event| event.score_total)
            .or_else(|| meta.and_then(|event| event.score_total));
        let score_key = score_total
            .map(score_bucket)
            .unwrap_or_else(|| "UNKNOWN".to_string());
        let ai_bucket = ai_reviews
            .get(setup_id)
            .and_then(|event| event.ai_confidence)
            .map(ai_confidence_bucket)
            .unwrap_or_else(|| "NO_AI".to_string());

        scenario_map.entry(scenario).or_default().push(r);
        regime_map.entry(regime).or_default().push(r);
        session_map.entry(session).or_default().push(r);
        ai_map.entry(ai_bucket).or_default().push(r);
        confirmation_map
            .entry(confirmation.to_string())
            .or_default()
            .push(r);
        confirmation_type_map
            .entry(confirmation_type)
            .or_default()
            .push(r);
        score_map.entry(score_key).or_default().push(r);
    }

    let outcome_count = wins + losses + timeouts;
    let win_rate = if outcome_count > 0 {
        wins as f64 / outcome_count as f64
    } else {
        0.0
    };
    let avg_r = if outcome_count > 0 {
        sum_r / outcome_count as f64
    } else {
        0.0
    };
    let profit_factor = if gross_loss > 0.0 {
        Some(gross_profit / gross_loss)
    } else {
        None
    };
    let max_drawdown_r = max_drawdown(&mut r_values);

    TriggerSummary {
        schema: TRIGGER_JOURNAL_SCHEMA,
        watching_setups,
        watching_triggered,
        scanner_triggers,
        watching_trigger_rate,
        outcomes: outcome_count,
        wins,
        losses,
        timeouts,
        data_gaps,
        win_rate,
        avg_r,
        expectancy: avg_r,
        profit_factor,
        max_drawdown_r,
        by_scenario: build_buckets(scenario_map),
        by_regime: build_buckets(regime_map),
        by_session: build_buckets(session_map),
        by_ai_confidence: build_buckets(ai_map),
        by_confirmation: build_buckets(confirmation_map),
        by_confirmation_type: build_buckets(confirmation_type_map),
        by_score: build_buckets(score_map),
        strategist_views,
        strategist_models: build_counts(strategist_models),
        strategist_primaries: build_counts(strategist_primaries),
    }
}

fn ai_confidence_bucket(confidence: f64) -> String {
    if !confidence.is_finite() {
        return "NO_AI".to_string();
    }
    if confidence < 0.60 {
        "<0.60".to_string()
    } else if confidence < 0.70 {
        "0.60-0.70".to_string()
    } else if confidence < 0.80 {
        "0.70-0.80".to_string()
    } else if confidence < 0.90 {
        "0.80-0.90".to_string()
    } else {
        ">=0.90".to_string()
    }
}

fn score_bucket(score: u8) -> String {
    match score {
        0..=49 => "<50".to_string(),
        50..=59 => "50-59".to_string(),
        60..=69 => "60-69".to_string(),
        70..=79 => "70-79".to_string(),
        80..=89 => "80-89".to_string(),
        _ => "90+".to_string(),
    }
}

fn max_drawdown(values: &mut [(DateTime<Utc>, f64)]) -> f64 {
    values.sort_by_key(|(at, _)| *at);
    let mut equity: f64 = 0.0;
    let mut peak: f64 = 0.0;
    let mut max_dd: f64 = 0.0;
    for (_, r) in values {
        equity += *r;
        peak = peak.max(equity);
        max_dd = max_dd.max(peak - equity);
    }
    max_dd
}

fn build_counts(groups: HashMap<String, usize>) -> Vec<CountBucket> {
    let mut buckets: Vec<CountBucket> = groups
        .into_iter()
        .map(|(key, count)| CountBucket { key, count })
        .collect();
    buckets.sort_by(|a, b| a.key.cmp(&b.key));
    buckets
}

fn build_buckets(groups: HashMap<String, Vec<f64>>) -> Vec<BucketSummary> {
    let mut buckets: Vec<BucketSummary> = groups
        .into_iter()
        .map(|(key, values)| {
            let wins = values.iter().filter(|value| **value > 0.0).count();
            let losses = values.iter().filter(|value| **value < 0.0).count();
            let timeouts = values.iter().filter(|value| **value == 0.0).count();
            let outcomes = values.len();
            let sum_r: f64 = values.iter().sum();
            let gross_profit: f64 = values.iter().filter(|value| **value > 0.0).sum();
            let gross_loss: f64 = values
                .iter()
                .filter(|value| **value < 0.0)
                .map(|value| value.abs())
                .sum();
            let win_rate = if outcomes > 0 {
                wins as f64 / outcomes as f64
            } else {
                0.0
            };
            let avg_r = if outcomes > 0 {
                sum_r / outcomes as f64
            } else {
                0.0
            };
            let profit_factor = if gross_loss > 0.0 {
                Some(gross_profit / gross_loss)
            } else {
                None
            };
            BucketSummary {
                key,
                outcomes,
                wins,
                losses,
                timeouts,
                win_rate,
                avg_r,
                expectancy: avg_r,
                profit_factor,
            }
        })
        .collect();
    buckets.sort_by(|a, b| a.key.cmp(&b.key));
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("trigger_journal_test_{stamp}.jsonl"))
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

    fn setup_event(setup_id: &str, scenario_id: &str, regime: &str, session: &str) -> TriggerEvent {
        let mut event = TriggerEvent::new("setup_created");
        event.setup_id = Some(setup_id.to_string());
        event.scenario_id = Some(scenario_id.to_string());
        event.regime = Some(regime.to_string());
        event.session = Some(session.to_string());
        event.confidence = Some(80);
        event
    }

    fn triggered_event(setup_id: &str, bias: &str, entry: f64, sl: f64, tp: f64) -> TriggerEvent {
        let mut event = TriggerEvent::new("triggered");
        event.setup_id = Some(setup_id.to_string());
        event.scenario_id = Some("bearish_breakdown".to_string());
        event.bias = Some(bias.to_string());
        event.entry = Some(entry);
        event.stop_loss = Some(sl);
        event.take_profit_1 = Some(tp);
        event.candle_timestamp = Some(0);
        event.confirmation_pass = Some(true);
        event.confirmation_type = Some("DIRECTIONAL_CLOSE".to_string());
        event.score_total = Some(80);
        event
    }

    fn outcome_event(setup_id: &str, outcome: &str, r: f64) -> TriggerEvent {
        let mut event = TriggerEvent::new("outcome");
        event.setup_id = Some(setup_id.to_string());
        event.outcome = Some(outcome.to_string());
        event.r_multiple = Some(r);
        event
    }

    #[test]
    fn regime_label_groups_trend_range_mixed() {
        assert_eq!(regime_label("BEARISH", "BEARISH", "BEARISH"), "TREND_BEAR");
        assert_eq!(regime_label("BULLISH", "BULLISH", "BULLISH"), "TREND_BULL");
        assert_eq!(regime_label("BEARISH", "BULLISH", "BULLISH"), "RANGE");
        assert_eq!(regime_label("BEARISH", "BEARISH", "BULLISH"), "MIXED");
    }

    #[test]
    fn append_and_load_roundtrip() {
        let path = temp_path();
        let journal = TriggerJournal::new(&path);
        journal
            .append(&setup_event("S1", "bearish_breakdown", "TREND_BEAR", "IN"))
            .expect("append");
        journal
            .append(&triggered_event("S1", "BEARISH", 100.0, 105.0, 90.0))
            .expect("append");
        let events = journal.load(None);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "setup_created");
        assert_eq!(events[1].event, "triggered");
        assert_eq!(events[1].schema, TRIGGER_JOURNAL_SCHEMA);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn telemetry_fields_roundtrip_and_legacy_defaults() {
        let mut event = TriggerEvent::new("decision_trace");
        event.decision_id = Some("D1".to_string());
        event.strategy_version = Some("2026-11-19.1".to_string());
        event.config_hash = Some("cfg".to_string());
        event.snapshot_hash = Some("snap".to_string());
        event.gate = Some("confirmation".to_string());
        event.gate_subreason = Some("volume".to_string());
        event.trigger_to_gate_ms = Some(1200);
        event.drift_at_gate = Some(0.4);
        event.score = Some(75);
        event.distance_ema_atr = Some(1.2);
        event.entry_type = Some("MARKET".to_string());
        event.rsi_h1 = Some(42.0);
        event.rsi_m15 = Some(55.0);
        event.sl_distance_atr = Some(1.1);
        event.atr_percentile = Some(65.0);
        event.wick_body_ratio = Some(1.8);
        event.shadow = Some(true);
        event.shadow_outcome = Some("TP1".to_string());
        event.shadow_r = Some(2.0);

        let json = serde_json::to_string(&event).expect("serialize telemetry");
        let decoded: TriggerEvent = serde_json::from_str(&json).expect("deserialize telemetry");
        assert_eq!(decoded.decision_id.as_deref(), Some("D1"));
        assert_eq!(decoded.gate.as_deref(), Some("confirmation"));
        assert_eq!(decoded.trigger_to_gate_ms, Some(1200));
        assert_eq!(decoded.entry_type.as_deref(), Some("MARKET"));
        assert_eq!(decoded.rsi_h1, Some(42.0));
        assert_eq!(decoded.atr_percentile, Some(65.0));
        assert_eq!(decoded.shadow_r, Some(2.0));

        let legacy: TriggerEvent =
            serde_json::from_str(r#"{"schema":1,"event":"triggered"}"#).expect("legacy event");
        assert!(legacy.decision_id.is_none());
        assert!(legacy.shadow.is_none());
    }

    #[test]
    fn blocked_triggers_are_marked_as_shadow() {
        let path = temp_path();
        let journal = TriggerJournal::new(&path);
        let mut confirmation_blocked = triggered_event("S1", "BEARISH", 100.0, 105.0, 90.0);
        confirmation_blocked.confirmation_pass = Some(false);
        journal.append(&confirmation_blocked).expect("append");
        journal
            .append(&triggered_event("S2", "BEARISH", 100.0, 105.0, 90.0))
            .expect("append");
        let mut ai_review = TriggerEvent::new("ai_review");
        ai_review.setup_id = Some("S2".to_string());
        ai_review.ai_blocked = Some(true);
        journal.append(&ai_review).expect("append");

        let open = journal.open_triggers();
        assert_eq!(open.len(), 2);
        assert!(open.iter().all(|trigger| trigger.shadow));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn open_triggers_excludes_completed_setups() {
        let path = temp_path();
        let journal = TriggerJournal::new(&path);
        journal
            .append(&triggered_event("S1", "BEARISH", 100.0, 105.0, 90.0))
            .expect("append");
        journal
            .append(&triggered_event("S2", "BEARISH", 100.0, 105.0, 90.0))
            .expect("append");
        journal
            .append(&outcome_event("S1", "TP1", 2.0))
            .expect("append");
        let open = journal.open_triggers();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].setup_id, "S2");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn sell_tp1_and_mfe_mae() {
        let trigger = OpenTrigger {
            setup_id: "S1".to_string(),
            signal_id: None,
            direction: "SELL".to_string(),
            entry: 100.0,
            stop_loss: 105.0,
            take_profit_1: 90.0,
            trigger_candle_timestamp: 0,
            triggered_at: Utc::now(),
            ..Default::default()
        };
        let candles = vec![
            candle(0, 101.0, 99.0, 100.0),
            candle(15 * 60 * 1000, 101.0, 89.0, 90.0),
        ];
        let OutcomeEvaluation::Complete(outcome) = evaluate_outcome(&trigger, &candles, 24) else {
            panic!("expected complete outcome");
        };
        assert_eq!(outcome.outcome, "TP1");
        assert!((outcome.r_multiple - 2.0).abs() < 1e-9);
        assert!((outcome.mfe_1_r.unwrap() - 2.2).abs() < 1e-9);
        assert!((outcome.mae_1_r.unwrap() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn same_candle_sl_wins_conservatively() {
        let trigger = OpenTrigger {
            setup_id: "S1".to_string(),
            signal_id: None,
            direction: "SELL".to_string(),
            entry: 100.0,
            stop_loss: 105.0,
            take_profit_1: 90.0,
            trigger_candle_timestamp: 0,
            triggered_at: Utc::now(),
            ..Default::default()
        };
        let candles = vec![
            candle(0, 101.0, 99.0, 100.0),
            candle(15 * 60 * 1000, 106.0, 89.0, 95.0),
        ];
        let OutcomeEvaluation::Complete(outcome) = evaluate_outcome(&trigger, &candles, 24) else {
            panic!("expected complete outcome");
        };
        assert_eq!(outcome.outcome, "SL");
        assert!((outcome.r_multiple + 1.0).abs() < 1e-9);
    }

    #[test]
    fn buy_tp1_works() {
        let trigger = OpenTrigger {
            setup_id: "S1".to_string(),
            signal_id: None,
            direction: "BUY".to_string(),
            entry: 100.0,
            stop_loss: 95.0,
            take_profit_1: 110.0,
            trigger_candle_timestamp: 0,
            triggered_at: Utc::now(),
            ..Default::default()
        };
        let candles = vec![
            candle(0, 101.0, 99.0, 100.0),
            candle(15 * 60 * 1000, 111.0, 99.0, 110.0),
        ];
        let OutcomeEvaluation::Complete(outcome) = evaluate_outcome(&trigger, &candles, 24) else {
            panic!("expected complete outcome");
        };
        assert_eq!(outcome.outcome, "TP1");
        assert!((outcome.r_multiple - 2.0).abs() < 1e-9);
    }

    #[test]
    fn timeout_uses_close_r() {
        let trigger = OpenTrigger {
            setup_id: "S1".to_string(),
            signal_id: None,
            direction: "SELL".to_string(),
            entry: 100.0,
            stop_loss: 105.0,
            take_profit_1: 80.0,
            trigger_candle_timestamp: 0,
            triggered_at: Utc::now(),
            ..Default::default()
        };
        let mut candles = vec![candle(0, 101.0, 99.0, 100.0)];
        for index in 1..=24 {
            candles.push(candle(index * 15 * 60 * 1000, 101.0, 99.0, 98.0));
        }
        let OutcomeEvaluation::Complete(outcome) = evaluate_outcome(&trigger, &candles, 24) else {
            panic!("expected complete outcome");
        };
        assert_eq!(outcome.outcome, "TIMEOUT");
        assert!((outcome.r_multiple - 0.4).abs() < 1e-9);
    }

    #[test]
    fn data_gap_when_trigger_candle_missing() {
        let trigger = OpenTrigger {
            setup_id: "S1".to_string(),
            signal_id: None,
            direction: "SELL".to_string(),
            entry: 100.0,
            stop_loss: 105.0,
            take_profit_1: 90.0,
            trigger_candle_timestamp: 999,
            triggered_at: Utc::now(),
            ..Default::default()
        };
        let candles = vec![candle(0, 101.0, 99.0, 100.0)];
        assert!(matches!(
            evaluate_outcome(&trigger, &candles, 24),
            OutcomeEvaluation::DataGap
        ));
    }

    #[test]
    fn summary_computes_trigger_rate_expectancy_and_buckets() {
        let mut events = vec![
            setup_event("S1", "bearish_breakdown", "TREND_BEAR", "IN"),
            setup_event("S2", "bearish_breakdown", "RANGE", "IN"),
            triggered_event("S1", "BEARISH", 100.0, 105.0, 90.0),
            triggered_event("S2", "BEARISH", 100.0, 105.0, 90.0),
            triggered_event("S3", "BEARISH", 100.0, 105.0, 90.0),
            outcome_event("S1", "TP1", 2.0),
            outcome_event("S2", "SL", -1.0),
            outcome_event("S3", "TIMEOUT", 0.0),
        ];
        let mut strategist = TriggerEvent::new("strategist_view");
        strategist.ai_model = Some("gemini-4-argon".to_string());
        strategist.scenario_id = Some("bearish_breakdown".to_string());
        events.push(strategist);
        let summary = summarize(&events);
        assert_eq!(summary.watching_setups, 2);
        assert_eq!(summary.watching_triggered, 2);
        assert_eq!(summary.scanner_triggers, 1);
        assert!((summary.watching_trigger_rate - 1.0).abs() < 1e-9);
        assert_eq!(summary.outcomes, 3);
        assert_eq!(summary.wins, 1);
        assert_eq!(summary.losses, 1);
        assert_eq!(summary.timeouts, 1);
        assert!((summary.avg_r - (1.0 / 3.0)).abs() < 1e-9);
        assert!((summary.profit_factor.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(summary.by_scenario.len(), 1);
        assert_eq!(summary.by_regime.len(), 3);
        assert_eq!(summary.by_confirmation_type.len(), 1);
        assert_eq!(summary.by_confirmation_type[0].key, "DIRECTIONAL_CLOSE");
        assert_eq!(summary.by_score.len(), 1);
        assert_eq!(summary.by_score[0].key, "80-89");
        assert_eq!(summary.strategist_views, 1);
        assert_eq!(summary.strategist_models[0].key, "gemini-4-argon");
        assert_eq!(summary.strategist_primaries[0].key, "bearish_breakdown");
    }
}
