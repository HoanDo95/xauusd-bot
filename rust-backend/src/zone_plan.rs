//! Zone-advisory reference profile.
//!
//! The bot is a market-view/alert tool. A zone plan uses a fixed 20-point
//! stop, a 20-point first target (50% partial), and a 40–50-point runner
//! target chosen from the structural scenario target. It never sizes a
//! position or checks the account risk budget.

/// Fixed stop-loss distance in gold price points.
pub const SL_POINTS: f64 = 20.0;
/// First-target distance in gold price points (50% partial).
pub const TP1_POINTS: f64 = 20.0;
/// Minimum runner-target distance in gold price points.
pub const TP2_MIN_POINTS: f64 = 40.0;
/// Maximum runner-target distance in gold price points.
pub const TP2_MAX_POINTS: f64 = 50.0;

/// Return `(stop_loss, take_profit_1, take_profit_2, tp2_points)`.
///
/// `scenario_tp2` is the structural target from the scenario. Its distance is
/// clamped into `40..=50`; when absent, the widest reference target is used.
pub fn levels(entry: f64, side: &str, scenario_tp2: Option<f64>) -> (f64, f64, f64, f64) {
    let is_buy = side.trim().eq_ignore_ascii_case("BUY");
    let tp2_points = scenario_tp2
        .map(|tp2| (tp2 - entry).abs())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(TP2_MAX_POINTS)
        .clamp(TP2_MIN_POINTS, TP2_MAX_POINTS);
    if is_buy {
        (
            entry - SL_POINTS,
            entry + TP1_POINTS,
            entry + tp2_points,
            tp2_points,
        )
    } else {
        (
            entry + SL_POINTS,
            entry - TP1_POINTS,
            entry - tp2_points,
            tp2_points,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buy_levels_use_fixed_sl_and_clamped_tp2() {
        assert_eq!(
            levels(100.0, "BUY", Some(130.0)),
            (80.0, 120.0, 140.0, 40.0)
        );
        assert_eq!(
            levels(100.0, "BUY", Some(200.0)),
            (80.0, 120.0, 150.0, 50.0)
        );
        assert_eq!(levels(100.0, "BUY", None), (80.0, 120.0, 150.0, 50.0));
    }

    #[test]
    fn sell_levels_mirror_buy_levels() {
        assert_eq!(levels(100.0, "SELL", Some(70.0)), (120.0, 80.0, 60.0, 40.0));
        assert_eq!(levels(100.0, "SELL", Some(0.0)), (120.0, 80.0, 50.0, 50.0));
        assert_eq!(levels(100.0, "SELL", Some(90.0)), (120.0, 80.0, 60.0, 40.0));
    }
}
