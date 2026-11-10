use crate::models::{BotConfig, SignalType};
use crate::risk_manager::PositionSizeResult;

/// Minimum allowed stop-loss distance in gold price points.
const MIN_RISK_POINTS: f64 = 3.0;
/// Maximum allowed stop-loss distance in gold price points.
const MAX_RISK_POINTS: f64 = 45.0;
/// TP2 must sit strictly beyond TP1 by more than one cent.
const MIN_TP2_GAP: f64 = 0.01;
/// Floating-point tolerance for the minimum R:R comparison.
const RR_EPSILON: f64 = 1e-9;

/// Validate the structural integrity of a signal's entry/SL/TP levels.
///
/// Rules:
/// - every price is finite and > 0;
/// - `risk = |entry - stop_loss|` is within `3.0..=45.0`;
/// - BUY: `stop_loss < entry < tp1 < tp2`; SELL is the mirror image;
/// - TP1 reward / risk is at least `min_risk_reward` (with 1e-9 epsilon);
/// - TP2 reward is strictly greater than TP1 reward by more than $0.01.
pub fn validate_signal_levels(
    signal_type: SignalType,
    entry_price: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    min_risk_reward: f64,
) -> Result<(), String> {
    for (label, price) in [
        ("entry_price", entry_price),
        ("stop_loss", stop_loss),
        ("take_profit_1", take_profit_1),
        ("take_profit_2", take_profit_2),
    ] {
        if !price.is_finite() || price <= 0.0 {
            return Err(format!("{label} must be finite and > 0"));
        }
    }

    let risk = (entry_price - stop_loss).abs();
    if !(MIN_RISK_POINTS..=MAX_RISK_POINTS).contains(&risk) {
        return Err(format!(
            "risk distance must be within {MIN_RISK_POINTS:.1}..={MAX_RISK_POINTS:.1} (got {risk:.2})"
        ));
    }

    match signal_type {
        SignalType::Buy => {
            if !(stop_loss < entry_price
                && entry_price < take_profit_1
                && take_profit_1 < take_profit_2)
            {
                return Err(
                    "BUY levels must satisfy stop_loss < entry_price < take_profit_1 < take_profit_2"
                        .to_string(),
                );
            }
        }
        SignalType::Sell => {
            if !(stop_loss > entry_price
                && entry_price > take_profit_1
                && take_profit_1 > take_profit_2)
            {
                return Err(
                    "SELL levels must satisfy stop_loss > entry_price > take_profit_1 > take_profit_2"
                        .to_string(),
                );
            }
        }
    }

    if !min_risk_reward.is_finite() || min_risk_reward <= 0.0 {
        return Err("min_risk_reward must be finite and > 0".to_string());
    }

    let tp1_reward = (take_profit_1 - entry_price).abs();
    let tp2_reward = (take_profit_2 - entry_price).abs();

    if tp1_reward / risk + RR_EPSILON < min_risk_reward {
        return Err(format!(
            "TP1 risk/reward {:.4} is below minimum {:.4}",
            tp1_reward / risk,
            min_risk_reward
        ));
    }

    if tp2_reward <= tp1_reward + MIN_TP2_GAP {
        return Err(format!(
            "take_profit_2 must be more than {MIN_TP2_GAP:.2} beyond take_profit_1 (tp1_reward {tp1_reward:.2}, tp2_reward {tp2_reward:.2})"
        ));
    }

    Ok(())
}

/// Validate that the computed position size respects the configured risk budget.
///
/// Rules:
/// - actual risk must not exceed `account_balance * risk_percentage / 100` by
///   more than 1% (rounding tolerance);
/// - the estimated margin must not exceed the account balance.
pub fn validate_risk_budget(config: &BotConfig, sizing: &PositionSizeResult) -> Result<(), String> {
    if !sizing.risk_amount_usd.is_finite() || !sizing.estimated_margin_usd.is_finite() {
        return Err("position sizing contains non-finite values".to_string());
    }

    let max_risk = config.account_balance * config.risk_percentage / 100.0;
    if !max_risk.is_finite() || max_risk < 0.0 {
        return Err("configured risk budget is not finite".to_string());
    }

    if sizing.risk_amount_usd > max_risk * 1.01 {
        return Err(format!(
            "risk_amount_usd {:.2} exceeds allowed risk {:.2} (balance {:.2} x {:.2}% + 1% tolerance)",
            sizing.risk_amount_usd, max_risk, config.account_balance, config.risk_percentage
        ));
    }

    if sizing.estimated_margin_usd > config.account_balance {
        return Err(format!(
            "estimated_margin_usd {:.2} exceeds account balance {:.2}",
            sizing.estimated_margin_usd, config.account_balance
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizing(risk_amount_usd: f64, estimated_margin_usd: f64) -> PositionSizeResult {
        PositionSizeResult {
            lot_size: 0.1,
            risk_amount_usd,
            potential_profit_tp1_usd: risk_amount_usd * 2.0,
            potential_profit_tp2_usd: risk_amount_usd * 3.0,
            estimated_margin_usd,
            risk_assessment: String::new(),
        }
    }

    #[test]
    fn valid_buy_levels_pass() {
        assert!(
            validate_signal_levels(SignalType::Buy, 2400.0, 2390.0, 2420.0, 2430.0, 2.0).is_ok()
        );
    }

    #[test]
    fn valid_sell_levels_pass() {
        assert!(
            validate_signal_levels(SignalType::Sell, 2400.0, 2410.0, 2380.0, 2370.0, 2.0).is_ok()
        );
    }

    #[test]
    fn reversed_levels_are_rejected() {
        let buy = validate_signal_levels(SignalType::Buy, 2400.0, 2410.0, 2380.0, 2370.0, 2.0)
            .expect_err("reversed BUY levels must fail");
        assert!(buy.contains("BUY levels"));

        let sell = validate_signal_levels(SignalType::Sell, 2400.0, 2390.0, 2420.0, 2430.0, 2.0)
            .expect_err("reversed SELL levels must fail");
        assert!(sell.contains("SELL levels"));
    }

    #[test]
    fn tp2_not_beyond_tp1_is_rejected() {
        let err = validate_signal_levels(SignalType::Buy, 2400.0, 2390.0, 2420.0, 2420.0, 2.0)
            .expect_err("TP2 must be strictly beyond TP1");
        assert!(err.contains("take_profit_2"));

        let err = validate_signal_levels(SignalType::Sell, 2400.0, 2410.0, 2380.0, 2380.0, 2.0)
            .expect_err("SELL TP2 must be strictly beyond TP1");
        assert!(err.contains("take_profit_2"));
    }

    #[test]
    fn risk_reward_below_minimum_is_rejected() {
        let err = validate_signal_levels(SignalType::Buy, 2400.0, 2390.0, 2410.0, 2420.0, 2.0)
            .expect_err("1R TP1 must fail a 2R minimum");
        assert!(err.contains("risk/reward"));
    }

    #[test]
    fn non_finite_and_zero_prices_are_rejected() {
        for bad in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
            assert!(
                validate_signal_levels(SignalType::Buy, bad, 2390.0, 2420.0, 2430.0, 2.0).is_err(),
                "entry {bad} must be rejected"
            );
        }

        assert!(
            validate_signal_levels(SignalType::Buy, 2400.0, 2390.0, f64::NAN, 2430.0, 2.0).is_err()
        );
    }

    #[test]
    fn risk_distance_outside_bounds_is_rejected() {
        // 2.99 points is below the 3.0 floor.
        let too_tight =
            validate_signal_levels(SignalType::Buy, 2400.0, 2397.01, 2420.0, 2430.0, 2.0)
                .expect_err("risk below 3 points must fail");
        assert!(too_tight.contains("risk distance"));

        // 45.01 points is above the 45.0 ceiling.
        let too_wide =
            validate_signal_levels(SignalType::Buy, 2400.0, 2354.99, 2420.0, 2430.0, 2.0)
                .expect_err("risk above 45 points must fail");
        assert!(too_wide.contains("risk distance"));
    }

    #[test]
    fn valid_risk_budget_passes() {
        let config = BotConfig::default(); // $10,000 balance, 1% => $100 max risk
        assert!(validate_risk_budget(&config, &sizing(100.0, 500.0)).is_ok());
        // 1% tolerance allows $101.
        assert!(validate_risk_budget(&config, &sizing(101.0, 500.0)).is_ok());
    }

    #[test]
    fn risk_budget_breach_is_rejected() {
        let config = BotConfig::default();
        let err = validate_risk_budget(&config, &sizing(102.0, 500.0))
            .expect_err("risk above budget + tolerance must fail");
        assert!(err.contains("exceeds allowed risk"));
    }

    #[test]
    fn margin_above_balance_is_rejected() {
        let config = BotConfig::default();
        let err = validate_risk_budget(&config, &sizing(50.0, 10_001.0))
            .expect_err("margin above balance must fail");
        assert!(err.contains("estimated_margin_usd"));
    }

    #[test]
    fn non_finite_sizing_is_rejected() {
        let config = BotConfig::default();
        assert!(validate_risk_budget(&config, &sizing(f64::NAN, 500.0)).is_err());
        assert!(validate_risk_budget(&config, &sizing(50.0, f64::INFINITY)).is_err());
    }
}
