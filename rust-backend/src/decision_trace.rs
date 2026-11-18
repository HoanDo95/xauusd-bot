//! Decision identity, versioning and reproducible snapshots.
//!
//! Every live/backtest decision can carry a `DecisionTrace` with a stable
//! `config_hash` and `snapshot_hash`. This is measurement-only: it never
//! changes signal generation, quota, Telegram or risk.

use crate::macro_flow::MacroFlowSnapshot;
use crate::market_context::MarketContext;
use crate::models::{BotConfig, Candle, Signal};
use crate::news_filter::EconomicEvent;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

/// Bump when decision semantics change. Keep this in journal/health/Telegram.
pub const STRATEGY_VERSION: &str = "2026-11-19.1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateResult {
    pub gate: String,
    pub passed: bool,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl GateResult {
    pub fn pass(gate: &str, reason: &str) -> Self {
        Self {
            gate: gate.to_string(),
            passed: true,
            reason: reason.to_string(),
            detail: None,
        }
    }

    pub fn block(gate: &str, reason: &str) -> Self {
        Self {
            gate: gate.to_string(),
            passed: false,
            reason: reason.to_string(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionTrace {
    pub decision_id: String,
    pub strategy_version: String,
    pub config_hash: String,
    pub snapshot_hash: String,
    pub as_of: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candle_timestamp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<u8>,
    pub gates: Vec<GateResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_to_gate_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift_at_gate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_ema_atr: Option<f64>,
    /// NO_CANDIDATE | BLOCKED | APPROVED
    pub final_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<Signal>,
}

impl DecisionTrace {
    pub fn new(
        now: DateTime<Utc>,
        config_hash: String,
        snapshot_hash: String,
        candle_timestamp: Option<i64>,
    ) -> Self {
        let decision_id = decision_id(now, &snapshot_hash);
        Self {
            decision_id,
            strategy_version: STRATEGY_VERSION.to_string(),
            config_hash,
            snapshot_hash,
            as_of: now,
            candle_timestamp,
            scenario_id: None,
            role: None,
            direction: None,
            confidence: None,
            score: None,
            gates: Vec::new(),
            trigger_to_gate_ms: None,
            drift_at_gate: None,
            distance_ema_atr: None,
            final_state: "NO_CANDIDATE".to_string(),
            signal: None,
        }
    }

    pub fn blocked_gate(&self) -> Option<&GateResult> {
        self.gates.iter().find(|gate| !gate.passed)
    }

    pub fn is_approved(&self) -> bool {
        self.final_state == "APPROVED"
    }
}

/// Stable config hash without secrets. Changing a strategy parameter changes
/// the hash; changing the Telegram token/passphrase does not.
pub fn config_hash(config: &BotConfig) -> String {
    let mut sanitized = config.clone();
    sanitized.telegram_bot_token.clear();
    sanitized.webhook_passphrase.clear();
    match serde_json::to_vec(&sanitized) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => sha256_hex(format!("{sanitized:?}").as_bytes()),
    }
}

/// Deterministic hash of the decision inputs. `updated_at` and other volatile
/// display metadata are intentionally excluded.
pub fn snapshot_hash(
    config_hash: &str,
    candles: &[Candle],
    context: Option<&MarketContext>,
    macro_flow: Option<&MacroFlowSnapshot>,
    news_events: &[EconomicEvent],
) -> String {
    let mut buffer = String::with_capacity(4096);
    let _ = writeln!(buffer, "config={config_hash}");
    let start = candles.len().saturating_sub(60);
    for candle in &candles[start..] {
        let _ = writeln!(
            buffer,
            "c:{}:{}:{}:{}:{}:{}",
            candle.timestamp,
            candle.open.to_bits(),
            candle.high.to_bits(),
            candle.low.to_bits(),
            candle.close.to_bits(),
            candle.volume.to_bits()
        );
    }
    if let Some(context) = context {
        let _ = writeln!(
            buffer,
            "ctx:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            context.d1.bias,
            context.d1.structure,
            context.d1.atr14.to_bits(),
            context.h4.bias,
            context.h4.structure,
            context.h4.atr14.to_bits(),
            context.h1.bias,
            context.h1.structure,
            context.h1.atr14.to_bits(),
            context
                .rsi_m15
                .as_ref()
                .and_then(|rsi| rsi.value)
                .unwrap_or(f64::NAN)
                .to_bits(),
            context
                .rsi_h1
                .as_ref()
                .and_then(|rsi| rsi.value)
                .unwrap_or(f64::NAN)
                .to_bits(),
            context
                .rsi_h4
                .as_ref()
                .and_then(|rsi| rsi.value)
                .unwrap_or(f64::NAN)
                .to_bits(),
            context.d1.close.to_bits()
        );
    }
    if let Some(macro_flow) = macro_flow {
        let scorecard = &macro_flow.scorecard;
        let _ = writeln!(
            buffer,
            "macro:{}:{}:{}:{}:{}:{}:{}:{}",
            scorecard.macro_bias,
            scorecard.confidence,
            scorecard.score,
            scorecard.direction,
            scorecard.flow_quality,
            scorecard.risk_regime,
            scorecard.rate_regime,
            scorecard.usd_regime
        );
    }
    let mut news: Vec<&EconomicEvent> = news_events
        .iter()
        .filter(|event| event.currency.eq_ignore_ascii_case("USD"))
        .collect();
    news.sort_by_key(|event| event.scheduled_time);
    for event in news.iter().take(10) {
        let _ = writeln!(
            buffer,
            "news:{}:{}:{}:{:?}:{}",
            event.id,
            event.title,
            event.scheduled_time.to_rfc3339(),
            event.impact,
            event.actual.as_deref().unwrap_or("")
        );
    }
    sha256_hex(buffer.as_bytes())
}

pub fn decision_id(now: DateTime<Utc>, snapshot_hash: &str) -> String {
    let short = snapshot_hash.get(..8).unwrap_or(snapshot_hash);
    format!("{}-{}", now.format("%Y%m%d-%H%M%S"), short)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use chrono::TimeZone;

    fn candle(ts: i64, close: f64) -> Candle {
        Candle {
            timestamp: ts,
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume: 100.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    #[test]
    fn config_hash_is_stable_and_ignores_secrets() {
        let base = BotConfig::default();
        let first = config_hash(&base);
        assert_eq!(first, config_hash(&base));

        let mut with_secret = base.clone();
        with_secret.telegram_bot_token = "secret-token".to_string();
        with_secret.webhook_passphrase = "secret-pass".to_string();
        assert_eq!(first, config_hash(&with_secret));

        let mut changed = base.clone();
        changed.min_risk_reward = 2.5;
        assert_ne!(first, config_hash(&changed));
    }

    #[test]
    fn snapshot_hash_is_deterministic_and_sensitive() {
        let config = BotConfig::default();
        let cfg_hash = config_hash(&config);
        let candles = vec![candle(1, 100.0), candle(2, 101.0)];
        let first = snapshot_hash(&cfg_hash, &candles, None, None, &[]);
        assert_eq!(first, snapshot_hash(&cfg_hash, &candles, None, None, &[]));

        let changed = vec![candle(1, 100.0), candle(2, 102.0)];
        assert_ne!(first, snapshot_hash(&cfg_hash, &changed, None, None, &[]));
    }

    #[test]
    fn decision_id_has_timestamp_and_short_hash() {
        let now = Utc.with_ymd_and_hms(2026, 11, 19, 1, 2, 3).unwrap();
        let id = decision_id(now, "abcdef0123456789");
        assert_eq!(id, "20261119-010203-abcdef01");
    }

    #[test]
    fn gate_result_serializes_optional_detail() {
        let gate = GateResult::block("confirmation", "confirmation_failed")
            .with_detail("volume 0.8x < 1.2x");
        let value = serde_json::to_value(&gate).expect("serialize gate");
        assert_eq!(value["gate"], "confirmation");
        assert_eq!(value["passed"], false);
        assert_eq!(value["detail"], "volume 0.8x < 1.2x");
    }
}
