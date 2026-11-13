//! Build a pending trade signal from a strong reference scenario.
//!
//! This is the deterministic bridge for "90% is enough": when no pinbar
//! candidate exists, the scanner may use a valid PRIMARY/ALTERNATE scenario
//! whose confidence clears the configured threshold. The entry is always the
//! scenario trigger (a pending order), never a market chase. The AI still has
//! to approve the direction and every hard risk rule still applies.

use crate::models::{BotConfig, Candle, EntryType, Signal, SignalStatus, SignalType};
use crate::risk_manager::PositionSizer;
use crate::scenarios::{Scenario, ScenarioResponse};
use crate::signal_validation::validate_signal_levels;
use chrono::{DateTime, Utc};

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn signal_type_for_bias(bias: &str) -> Option<SignalType> {
    match bias.trim().to_uppercase().as_str() {
        "BULLISH" => Some(SignalType::Buy),
        "BEARISH" => Some(SignalType::Sell),
        _ => None,
    }
}

fn entry_type_for_trigger(direction: &str) -> Option<EntryType> {
    match direction.trim().to_uppercase().as_str() {
        "ABOVE" | "BELOW" => Some(EntryType::Breakout),
        "TOUCH" => Some(EntryType::Limit),
        _ => None,
    }
}

/// Pick the strongest valid PRIMARY/ALTERNATE scenario that clears the
/// early-entry confidence threshold. `rank_scenarios` already orders roles and
/// priority, so the first match is the intended candidate.
pub fn best_early_entry_scenario(
    scenarios: &ScenarioResponse,
    min_confidence: u8,
) -> Option<&Scenario> {
    scenarios.scenarios.iter().find(|scenario| {
        scenario.valid
            && scenario.confidence >= min_confidence
            && matches!(
                scenario.role.trim().to_uppercase().as_str(),
                "PRIMARY" | "ALTERNATE"
            )
            && scenario.entry.is_some()
            && scenario.stop_loss.is_some()
            && scenario.take_profit_1.is_some()
            && scenario.take_profit_2.is_some()
    })
}

/// Convert a valid scenario into a pending `Signal`.
///
/// The signal is deliberately marked `is_entry_filled=false` and uses
/// BREAKOUT/LIMIT entry types so Telegram and the tracker show it as pending.
/// Returns `None` when levels are invalid, the bias is not directional, or the
/// trigger direction cannot be represented as a pending order.
pub fn signal_from_scenario(
    scenario: &Scenario,
    config: &BotConfig,
    candles: &[Candle],
    live_price: Option<f64>,
    now: DateTime<Utc>,
) -> Option<Signal> {
    let signal_type = signal_type_for_bias(&scenario.bias)?;
    let entry_type = entry_type_for_trigger(&scenario.trigger_direction)?;
    let entry = scenario
        .entry
        .filter(|value| value.is_finite() && *value > 0.0)?;
    let stop_loss = scenario
        .stop_loss
        .filter(|value| value.is_finite() && *value > 0.0)?;
    let take_profit_1 = scenario
        .take_profit_1
        .filter(|value| value.is_finite() && *value > 0.0)?;
    let take_profit_2 = scenario
        .take_profit_2
        .filter(|value| value.is_finite() && *value > 0.0)?;

    validate_signal_levels(
        signal_type,
        entry,
        stop_loss,
        take_profit_1,
        take_profit_2,
        config.min_risk_reward,
    )
    .ok()?;

    let last_candle = candles.last()?;
    let risk_points = round2((entry - stop_loss).abs());
    let reward_points_tp1 = round2((take_profit_1 - entry).abs());
    let reward_points_tp2 = round2((take_profit_2 - entry).abs());
    if risk_points <= 0.0 {
        return None;
    }
    let risk_reward_ratio = round2(reward_points_tp1 / risk_points);
    let sizing = PositionSizer::calculate_lot_size(
        config.account_balance,
        config.risk_percentage,
        risk_points,
        reward_points_tp1,
        reward_points_tp2,
        entry,
        config.account_leverage,
    );

    let role = scenario.role.trim().to_uppercase();
    let dedup_key = format!(
        "{}_{}_SCENARIO_{}_{:.2}",
        config.symbol, config.timeframe, scenario.id, entry
    );
    let trigger_price = scenario
        .trigger_price
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| format!("{entry:.2}"));

    Some(Signal {
        id: format!(
            "SIG-{}-{}",
            now.timestamp_millis(),
            scenario.id.to_uppercase()
        ),
        symbol: config.symbol.clone(),
        signal_type,
        timeframe: config.timeframe,
        entry_price: entry,
        stop_loss,
        take_profit_1,
        take_profit_2,
        risk_points,
        reward_points_tp1,
        reward_points_tp2,
        risk_reward_ratio,
        volume_surge_ratio: 0.0,
        recommended_lot_size: sizing.lot_size,
        risk_amount_usd: sizing.risk_amount_usd,
        potential_profit_tp1_usd: sizing.potential_profit_tp1_usd,
        potential_profit_tp2_usd: sizing.potential_profit_tp2_usd,
        price_action_notes: format!(
            "Kịch bản {} ({}), confidence {}% — entry tại trigger {}, chưa xác nhận. {}",
            scenario.name, role, scenario.confidence, trigger_price, scenario.condition
        ),
        volume_analysis_notes:
            "Scenario-based entry — volume PAXG chỉ là proxy tương đối, không phải volume XAU/USD thật."
                .to_string(),
        full_rationale: format!(
            "{} · Trigger: {} · Invalidation: {} · R:R 1:{:.2}. {}",
            scenario.name,
            scenario.trigger,
            scenario.invalidation,
            risk_reward_ratio,
            scenario.notes.join(" ")
        ),
        status: SignalStatus::Active,
        created_at: now,
        telegram_message_id: None,
        dedup_key,
        candle_timestamp: last_candle.timestamp,
        is_simulated: false,
        confluence_score: scenario.confidence,
        outcome_at: None,
        entry_type,
        candle_close_price: round2(last_candle.close),
        live_market_price: live_price,
        retest_limit_price: None,
        data_source: config.data_source.clone(),
        is_entry_filled: false,
        ai_verdict: None,
        ai_confidence: None,
        ai_reason: None,
        delta_ratio: None,
        cvd: None,
        delta: None,
        rsi_value: None,
        rsi_state: None,
        rsi_divergence: None,
        macro_flow_alignment: Some(scenario.macro_alignment.clone()),
        macro_flow_note: Some(format!(
            "Scenario {} · confidence {}% · entry tại trigger, chờ xác nhận",
            role, scenario.confidence
        )),
        cancel_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use crate::scenarios::ScenarioMarket;

    fn config() -> BotConfig {
        BotConfig {
            account_balance: 10_000.0,
            risk_percentage: 1.0,
            account_leverage: 100.0,
            min_risk_reward: 2.0,
            ..BotConfig::default()
        }
    }

    fn candle(timestamp: i64, close: f64) -> Candle {
        Candle {
            timestamp,
            open: close,
            high: close,
            low: close,
            close,
            volume: 1000.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn scenario(
        id: &str,
        role: &str,
        bias: &str,
        confidence: u8,
        entry: f64,
        stop_loss: f64,
        take_profit_1: f64,
        take_profit_2: f64,
        direction: &str,
    ) -> Scenario {
        Scenario {
            id: id.to_string(),
            name: id.to_string(),
            bias: bias.to_string(),
            trigger: "trigger".to_string(),
            condition: "condition".to_string(),
            entry: Some(entry),
            stop_loss: Some(stop_loss),
            take_profit_1: Some(take_profit_1),
            take_profit_2: Some(take_profit_2),
            risk_reward: Some((take_profit_1 - entry).abs() / (entry - stop_loss).abs()),
            trigger_price: Some(entry),
            trigger_direction: direction.to_string(),
            trigger_timeframe: "M15".to_string(),
            invalidation: "invalidation".to_string(),
            confidence,
            macro_alignment: "ALIGNED".to_string(),
            valid: true,
            role: role.to_string(),
            priority: i32::from(confidence),
            notes: vec![],
        }
    }

    fn response(scenarios: Vec<Scenario>) -> ScenarioResponse {
        ScenarioResponse {
            status: "ok".to_string(),
            generated_at: "2026-11-12T00:00:00Z".to_string(),
            price: Some(3900.0),
            source: "TEST".to_string(),
            volume_source: "TEST".to_string(),
            market: Some(ScenarioMarket {
                h4_bias: "BEARISH".to_string(),
                h1_bias: "BEARISH".to_string(),
                d1_bias: "BEARISH".to_string(),
                h4_atr: 20.0,
                nearest_support: Some(3880.0),
                nearest_resistance: Some(3920.0),
                rsi_m15: Some(40.0),
                rsi_state: "Neutral".to_string(),
                macro_bias: "BEARISH".to_string(),
                macro_confidence: 90,
                daily_change_pct: None,
                rsi_h1: None,
                rsi_h4: None,
            }),
            scenarios,
            warnings: vec![],
        }
    }

    #[test]
    fn picks_primary_at_or_above_threshold_and_ignores_weak_alternate() {
        let response = response(vec![
            scenario(
                "bearish_breakdown",
                "PRIMARY",
                "BEARISH",
                90,
                3880.0,
                3895.0,
                3850.0,
                3835.0,
                "BELOW",
            ),
            scenario(
                "bullish_bounce",
                "ALTERNATE",
                "BULLISH",
                60,
                3900.0,
                3885.0,
                3930.0,
                3945.0,
                "TOUCH",
            ),
        ]);

        let picked = best_early_entry_scenario(&response, 90).expect("primary scenario");
        assert_eq!(picked.id, "bearish_breakdown");
        assert!(best_early_entry_scenario(&response, 95).is_none());
    }

    #[test]
    fn ignores_watchlist_even_with_high_confidence() {
        let response = response(vec![scenario(
            "bearish_rejection",
            "WATCHLIST",
            "BEARISH",
            95,
            3900.0,
            3915.0,
            3870.0,
            3855.0,
            "TOUCH",
        )]);
        assert!(best_early_entry_scenario(&response, 90).is_none());
    }

    #[test]
    fn builds_pending_breakout_signal_at_scenario_trigger() {
        let scenario = scenario(
            "bearish_breakdown",
            "PRIMARY",
            "BEARISH",
            90,
            3880.0,
            3895.0,
            3850.0,
            3835.0,
            "BELOW",
        );
        let candles = vec![candle(1, 3900.0), candle(2, 3898.0)];
        let signal = signal_from_scenario(&scenario, &config(), &candles, Some(3898.0), Utc::now())
            .expect("signal");

        assert_eq!(signal.signal_type, SignalType::Sell);
        assert_eq!(signal.entry_price, 3880.0);
        assert_eq!(signal.stop_loss, 3895.0);
        assert_eq!(signal.entry_type, EntryType::Breakout);
        assert!(!signal.is_entry_filled);
        assert_eq!(signal.confluence_score, 90);
        assert_eq!(signal.candle_timestamp, 2);
        assert!(signal.dedup_key.contains("SCENARIO_bearish_breakdown"));
        assert!(signal.price_action_notes.contains("chưa xác nhận"));
    }

    #[test]
    fn touch_scenario_uses_limit_entry() {
        let scenario = scenario(
            "bullish_bounce",
            "ALTERNATE",
            "BULLISH",
            90,
            3880.0,
            3865.0,
            3910.0,
            3925.0,
            "TOUCH",
        );
        let signal = signal_from_scenario(
            &scenario,
            &config(),
            &[candle(1, 3880.0)],
            Some(3880.0),
            Utc::now(),
        )
        .expect("signal");
        assert_eq!(signal.entry_type, EntryType::Limit);
        assert!(!signal.is_entry_filled);
    }

    #[test]
    fn rejects_invalid_levels() {
        let mut scenario = scenario(
            "bearish_breakdown",
            "PRIMARY",
            "BEARISH",
            90,
            3880.0,
            3895.0,
            3850.0,
            3835.0,
            "BELOW",
        );
        scenario.take_profit_1 = Some(3879.0); // R:R far below 2
        assert!(signal_from_scenario(
            &scenario,
            &config(),
            &[candle(1, 3880.0)],
            Some(3880.0),
            Utc::now(),
        )
        .is_none());
    }
}
