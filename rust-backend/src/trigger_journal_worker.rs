//! Background worker that closes open trigger-journal entries with a
//! hypothetical TP1/SL/TIMEOUT outcome and MFE/MAE checkpoints.
//!
//! This runs independently from the live signal tracker so a trigger that was
//! rejected by confirmation/AI/risk is still measured. It is analysis-only and
//! never sends Telegram or changes a signal.

use crate::api::AppState;
use crate::trigger_journal::{
    evaluate_outcome, record, OutcomeEvaluation, TriggerEvent, TRIGGER_OUTCOME_MAX_BARS,
};
use std::time::Duration;
use tracing::{info, warn};

const TRIGGER_JOURNAL_INTERVAL_SECS: u64 = 60;

pub fn spawn_trigger_journal_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "📓 [Trigger Journal] Worker outcome đã khởi chạy (chu kỳ {} giây)...",
            TRIGGER_JOURNAL_INTERVAL_SECS
        );
        let mut interval =
            tokio::time::interval(Duration::from_secs(TRIGGER_JOURNAL_INTERVAL_SECS));
        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.trigger_journal_enabled {
                continue;
            }
            let candles = state.candles.read().await.clone();
            if candles.is_empty() {
                continue;
            }
            for trigger in state.trigger_journal.open_triggers() {
                let event = match evaluate_outcome(&trigger, &candles, TRIGGER_OUTCOME_MAX_BARS) {
                    OutcomeEvaluation::Pending => continue,
                    OutcomeEvaluation::DataGap => {
                        warn!(
                            "⚠️ [Trigger Journal] {} không tìm thấy nến trigger trong history — DATA_GAP.",
                            trigger.setup_id
                        );
                        let mut event = TriggerEvent::new("outcome");
                        event.setup_id = Some(trigger.setup_id.clone());
                        event.signal_id = trigger.signal_id.clone();
                        event.outcome = Some("DATA_GAP".to_string());
                        event.r_multiple = Some(0.0);
                        event.bars_to_outcome = Some(0);
                        event.reason =
                            Some("trigger candle not found in candle history".to_string());
                        event.decision_id = trigger.decision_id.clone();
                        event.strategy_version = trigger.strategy_version.clone();
                        event.config_hash = trigger.config_hash.clone();
                        event.snapshot_hash = trigger.snapshot_hash.clone();
                        event.gate = trigger.gate.clone();
                        event.gate_subreason = trigger.gate_subreason.clone();
                        event.score = trigger.score;
                        event.trigger_to_gate_ms = trigger.trigger_to_gate_ms;
                        event.drift_at_gate = trigger.drift_at_gate;
                        event.distance_ema_atr = trigger.distance_ema_atr;
                        event.shadow = Some(trigger.shadow);
                        event.shadow_outcome = Some("DATA_GAP".to_string());
                        event.shadow_r = Some(0.0);
                        event
                    }
                    OutcomeEvaluation::Complete(outcome) => {
                        let mut event = TriggerEvent::new("outcome");
                        event.setup_id = Some(trigger.setup_id.clone());
                        event.signal_id = trigger.signal_id.clone();
                        event.outcome = Some(outcome.outcome);
                        event.r_multiple = Some(outcome.r_multiple);
                        event.bars_to_outcome = Some(outcome.bars_to_outcome);
                        event.mfe_1_r = outcome.mfe_1_r;
                        event.mae_1_r = outcome.mae_1_r;
                        event.mfe_4_r = outcome.mfe_4_r;
                        event.mae_4_r = outcome.mae_4_r;
                        event.mfe_8_r = outcome.mfe_8_r;
                        event.mae_8_r = outcome.mae_8_r;
                        event.mfe_24_r = outcome.mfe_24_r;
                        event.mae_24_r = outcome.mae_24_r;
                        event.decision_id = trigger.decision_id.clone();
                        event.strategy_version = trigger.strategy_version.clone();
                        event.config_hash = trigger.config_hash.clone();
                        event.snapshot_hash = trigger.snapshot_hash.clone();
                        event.gate = trigger.gate.clone();
                        event.gate_subreason = trigger.gate_subreason.clone();
                        event.score = trigger.score;
                        event.trigger_to_gate_ms = trigger.trigger_to_gate_ms;
                        event.drift_at_gate = trigger.drift_at_gate;
                        event.distance_ema_atr = trigger.distance_ema_atr;
                        event.shadow = Some(trigger.shadow);
                        event.shadow_outcome = event.outcome.clone();
                        event.shadow_r = event.r_multiple;
                        event
                    }
                };
                record(&state.trigger_journal, true, &event);
            }
        }
    });
}
