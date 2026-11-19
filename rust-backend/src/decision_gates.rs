//! Shared deterministic decision gates for live and backtest.
//!
//! This module is intentionally pure: it takes candles/scenario/config and
//! returns a `GateEvaluation`. Live and backtest must call the same functions
//! so the measured gate pass rate matches production.

use crate::decision_trace::GateResult;
use crate::models::{BotConfig, Candle, Signal};
use crate::scenario_signal::{confirmed_signal_from_scenario, rsi_extreme_blocks_confirmed};
use crate::scenarios::Scenario;
use crate::tracker::{pending_confirmation_detail, ConfirmationType};
use chrono::{DateTime, Utc};
use tracing::info;

#[derive(Debug, Clone)]
pub struct GateEvaluation {
    pub signal: Option<Signal>,
    pub reason: String,
    pub confirmation_type: Option<ConfirmationType>,
    pub detail: Option<String>,
    pub gates: Vec<GateResult>,
}

impl GateEvaluation {
    pub fn blocked(&self) -> bool {
        self.signal.is_none()
    }

    pub fn first_block(&self) -> Option<&GateResult> {
        self.gates.iter().find(|gate| !gate.passed)
    }
}

/// Deterministic gates for one scenario on a closed candle.
///
/// Order is intentional: RSI extreme first, then level validity, then the
/// optional trigger confirmation. The returned `reason` preserves the legacy
/// journal labels (`rsi_extreme`, `invalid_levels`, `confirmation_failed`,
/// `confirmed`) for backward compatibility.
pub fn confirmed_signal_with_gates(
    scenario: &Scenario,
    config: &BotConfig,
    candles: &[Candle],
    live_price: Option<f64>,
    rsi_value: Option<f64>,
    now: DateTime<Utc>,
) -> GateEvaluation {
    let mut gates = Vec::with_capacity(4);

    if rsi_extreme_blocks_confirmed(
        &scenario.bias,
        rsi_value,
        config.rsi_oversold,
        config.rsi_overbought,
    ) {
        info!(
            "⏳ [Confirmed Signal] {} bị chặn: RSI M15 {:.1} đã quá bán/quá mua — chờ hồi, không đuổi.",
            scenario.id,
            rsi_value.unwrap_or_default()
        );
        gates.push(
            GateResult::block("rsi_extreme", "rsi_extreme")
                .with_detail(format!("RSI M15 {}", rsi_value.unwrap_or_default())),
        );
        return GateEvaluation {
            signal: None,
            reason: "rsi_extreme".to_string(),
            confirmation_type: None,
            detail: None,
            gates,
        };
    }
    gates.push(GateResult::pass("rsi_extreme", "not_extreme"));

    let Some(mut signal) =
        confirmed_signal_from_scenario(scenario, config, candles, live_price, now)
    else {
        info!(
            "⏳ [Confirmed Signal] {} levels không hợp lệ hoặc giá đã đi quá xa entry — bỏ qua.",
            scenario.id
        );
        gates.push(GateResult::block("levels", "invalid_levels"));
        return GateEvaluation {
            signal: None,
            reason: "invalid_levels".to_string(),
            confirmation_type: None,
            detail: None,
            gates,
        };
    };
    gates.push(GateResult::pass("levels", "valid"));

    let last_index = candles.len().saturating_sub(1);
    let mut confirmation_type = None;
    if config.trigger_confirmation_enabled {
        match pending_confirmation_detail(
            &signal,
            candles,
            last_index,
            config.quantum_volume_confirm_ratio,
        ) {
            (Some(kind), _) => {
                signal.management_note = Some(format!("M15 confirmation: {}", kind.as_str()));
                confirmation_type = Some(kind);
                gates.push(
                    GateResult::pass("confirmation", "confirmed")
                        .with_detail(kind.as_str().to_string()),
                );
            }
            (None, detail) => {
                info!(
                    "⏳ [Confirmed Signal] {} trigger chưa xác nhận ({}).",
                    scenario.id, detail
                );
                gates.push(
                    GateResult::block("confirmation", "confirmation_failed")
                        .with_detail(detail.clone()),
                );
                return GateEvaluation {
                    signal: None,
                    reason: "confirmation_failed".to_string(),
                    confirmation_type: None,
                    detail: Some(detail),
                    gates,
                };
            }
        }
    } else {
        gates.push(GateResult::pass("confirmation", "disabled"));
    }

    info!(
        "🚨 [Confirmed Signal] Scenario {} ({}) confidence {}% — entry {:.2} MARKET sau M15 close {:.2}.",
        scenario.role,
        scenario.id,
        scenario.confidence,
        signal.entry_price,
        candles.last().map(|candle| candle.close).unwrap_or_default()
    );
    GateEvaluation {
        signal: Some(signal),
        reason: "confirmed".to_string(),
        confirmation_type,
        detail: None,
        gates,
    }
}

/// Pre-candidate hard gates shared by live/backtest. The caller supplies
/// already-computed booleans so this stays pure and testable.
#[derive(Debug, Clone, Copy)]
pub struct PreGateInput {
    pub weekend: bool,
    pub session_in: bool,
    pub news_safe: bool,
    pub spread_proxy_atr: Option<f64>,
    pub max_spread_proxy_atr: f64,
    pub duplicate_candle: bool,
    pub quota_reached: bool,
    pub setup_owns_signal: bool,
}

pub fn evaluate_pre_gates(input: PreGateInput) -> Vec<GateResult> {
    let mut gates = Vec::with_capacity(8);
    gates.push(if input.weekend {
        GateResult::block("weekend", "market_closed_weekend")
    } else {
        GateResult::pass("weekend", "open")
    });
    gates.push(if input.session_in {
        GateResult::pass("session", "in_session")
    } else {
        GateResult::block("session", "outside_session")
    });
    gates.push(if input.news_safe {
        GateResult::pass("news", "no_high_impact_window")
    } else {
        GateResult::block("news", "high_impact_window")
    });
    if let Some(spread) = input.spread_proxy_atr {
        gates.push(if spread > input.max_spread_proxy_atr {
            GateResult::block("spread", "spread_proxy_too_high").with_detail(format!(
                "{spread:.2}xATR > {:.2}",
                input.max_spread_proxy_atr
            ))
        } else {
            GateResult::pass("spread", "within_limit")
        });
    } else {
        gates.push(GateResult::pass("spread", "unavailable"));
    }
    gates.push(if input.duplicate_candle {
        GateResult::block("duplicate_candle", "already_evaluated")
    } else {
        GateResult::pass("duplicate_candle", "fresh")
    });
    gates.push(if input.quota_reached {
        GateResult::block("quota", "daily_quota_reached")
    } else {
        GateResult::pass("quota", "available")
    });
    gates.push(if input.setup_owns_signal {
        GateResult::block("setup_ownership", "active_setup_owns_signal")
    } else {
        GateResult::pass("setup_ownership", "free")
    });
    gates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use chrono::TimeZone;

    fn candle(open: f64, high: f64, low: f64, close: f64, volume: f64) -> Candle {
        Candle {
            timestamp: 1,
            open,
            high,
            low,
            close,
            volume,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    fn scenario(bias: &str) -> Scenario {
        let (entry, sl, tp1, tp2, direction) = if bias == "BEARISH" {
            (3800.0, 3810.0, 3780.0, 3770.0, "BELOW")
        } else {
            (3800.0, 3790.0, 3820.0, 3830.0, "ABOVE")
        };
        Scenario {
            id: "test".to_string(),
            name: "Test".to_string(),
            bias: bias.to_string(),
            trigger: "test".to_string(),
            condition: "test".to_string(),
            entry: Some(entry),
            stop_loss: Some(sl),
            take_profit_1: Some(tp1),
            take_profit_2: Some(tp2),
            risk_reward: Some(2.0),
            trigger_price: Some(entry),
            trigger_direction: direction.to_string(),
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

    fn config() -> BotConfig {
        BotConfig {
            account_balance: 10_000.0,
            risk_percentage: 1.0,
            account_leverage: 100.0,
            min_risk_reward: 2.0,
            trigger_confirmation_enabled: false,
            ..BotConfig::default()
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 19, 0, 0, 0).unwrap()
    }

    #[test]
    fn rsi_extreme_blocks_sell_breakdown() {
        let candles = vec![candle(3800.0, 3801.0, 3790.0, 3791.0, 1000.0)];
        let result = confirmed_signal_with_gates(
            &scenario("BEARISH"),
            &config(),
            &candles,
            Some(3791.0),
            Some(20.0),
            now(),
        );
        assert!(result.blocked());
        assert_eq!(result.reason, "rsi_extreme");
        assert!(result
            .gates
            .iter()
            .any(|gate| gate.gate == "rsi_extreme" && !gate.passed));
    }

    #[test]
    fn invalid_levels_are_blocked() {
        let mut scenario = scenario("BEARISH");
        scenario.take_profit_2 = None;
        let candles = vec![candle(3800.0, 3801.0, 3790.0, 3791.0, 1000.0)];
        let result = confirmed_signal_with_gates(
            &scenario,
            &config(),
            &candles,
            Some(3791.0),
            Some(50.0),
            now(),
        );
        assert!(result.blocked());
        assert_eq!(result.reason, "invalid_levels");
    }

    #[test]
    fn confirmation_disabled_passes_valid_levels() {
        let candles = vec![candle(3800.0, 3801.0, 3790.0, 3791.0, 1000.0)];
        let result = confirmed_signal_with_gates(
            &scenario("BEARISH"),
            &config(),
            &candles,
            Some(3791.0),
            Some(50.0),
            now(),
        );
        assert!(result.signal.is_some());
        assert_eq!(result.reason, "confirmed");
    }

    #[test]
    fn pre_gates_report_each_blocker() {
        let gates = evaluate_pre_gates(PreGateInput {
            weekend: true,
            session_in: false,
            news_safe: false,
            spread_proxy_atr: Some(1.5),
            max_spread_proxy_atr: 1.0,
            duplicate_candle: true,
            quota_reached: true,
            setup_owns_signal: true,
        });
        let blocked: Vec<&str> = gates
            .iter()
            .filter(|gate| !gate.passed)
            .map(|gate| gate.gate.as_str())
            .collect();
        assert_eq!(
            blocked,
            vec![
                "weekend",
                "session",
                "news",
                "spread",
                "duplicate_candle",
                "quota",
                "setup_ownership"
            ]
        );
    }
}
