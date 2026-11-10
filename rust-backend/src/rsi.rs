//! RSI (Wilder) and RSI/price divergence primitives.
//!
//! This module is deliberately side-effect free: it does not change live signal
//! generation by itself. The backtest binary (Phase R0) consumes it to evaluate
//! RSI extreme and divergence gates.

use crate::models::Candle;
use serde::{Deserialize, Serialize};

/// Wilder-smoothed RSI series.
///
/// The first value is available after `period` candles (at index `period`);
/// earlier entries are `None`. A zero average loss yields 100, a zero average
/// gain yields 0, and a perfectly flat series yields 50.
pub fn rsi_series(candles: &[Candle], period: usize) -> Vec<Option<f64>> {
    let mut series = vec![None; candles.len()];
    if period == 0 || candles.len() <= period {
        return series;
    }

    let mut gain_sum = 0.0;
    let mut loss_sum = 0.0;
    for pair in candles[..=period].windows(2) {
        let change = pair[1].close - pair[0].close;
        if change > 0.0 {
            gain_sum += change;
        } else {
            loss_sum -= change;
        }
    }

    let period_f = period as f64;
    let mut avg_gain = gain_sum / period_f;
    let mut avg_loss = loss_sum / period_f;
    series[period] = Some(rsi_from_averages(avg_gain, avg_loss));

    for (offset, pair) in candles[period..].windows(2).enumerate() {
        let index = period + 1 + offset;
        let change = pair[1].close - pair[0].close;
        let (gain, loss) = if change > 0.0 {
            (change, 0.0)
        } else {
            (0.0, -change)
        };

        // Wilder smoothing: new average = (previous * (period - 1) + current) / period.
        avg_gain = (avg_gain * (period_f - 1.0) + gain) / period_f;
        avg_loss = (avg_loss * (period_f - 1.0) + loss) / period_f;
        series[index] = Some(rsi_from_averages(avg_gain, avg_loss));
    }

    series
}

fn rsi_from_averages(avg_gain: f64, avg_loss: f64) -> f64 {
    if avg_loss <= f64::EPSILON {
        if avg_gain <= f64::EPSILON {
            50.0
        } else {
            100.0
        }
    } else if avg_gain <= f64::EPSILON {
        0.0
    } else {
        let relative_strength = avg_gain / avg_loss;
        100.0 - 100.0 / (1.0 + relative_strength)
    }
}

/// RSI/price divergence direction over the recent pivot structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RsiDivergence {
    None,
    Bullish,
    Bearish,
}

/// Point-in-time RSI snapshot used by reporting or a future live gate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsiSnapshot {
    pub value: Option<f64>,
    pub state: String,
    pub divergence: RsiDivergence,
}

/// Classify an RSI value against fixed thresholds.
pub fn rsi_state(value: f64, overbought: f64, oversold: f64) -> String {
    if value >= overbought {
        "Overbought".to_string()
    } else if value <= oversold {
        "Oversold".to_string()
    } else {
        "Neutral".to_string()
    }
}

/// Detect regular RSI/price divergence from confirmed pivots.
///
/// Only candles present in the supplied slice are considered, so callers must
/// pass closed candles only. A pivot high needs `pivot_k` strictly lower highs
/// on each side; a pivot low needs `pivot_k` strictly higher lows on each side.
///
/// Bearish: the last two pivot highs are at least three candles apart, price
/// makes a higher high, and RSI makes a lower high. Bullish: the last two pivot
/// lows are at least three candles apart, price makes a lower low, and RSI makes
/// a higher low. If both patterns are present, the more recent pivot wins.
pub fn detect_divergence(
    candles: &[Candle],
    rsi: &[Option<f64>],
    lookback: usize,
    pivot_k: usize,
) -> RsiDivergence {
    const MIN_PIVOT_SEPARATION: usize = 3;

    if lookback == 0 || pivot_k == 0 || candles.len() <= 2 * pivot_k || rsi.len() < candles.len() {
        return RsiDivergence::None;
    }

    let start = candles.len().saturating_sub(lookback);
    let end = candles.len() - pivot_k;
    let mut pivot_highs = Vec::new();
    let mut pivot_lows = Vec::new();

    for (index, candle) in candles.iter().enumerate().take(end).skip(start + pivot_k) {
        let is_high = (1..=pivot_k).all(|offset| {
            candle.high > candles[index - offset].high && candle.high > candles[index + offset].high
        });
        let is_low = (1..=pivot_k).all(|offset| {
            candle.low < candles[index - offset].low && candle.low < candles[index + offset].low
        });

        if is_high {
            pivot_highs.push(index);
        }
        if is_low {
            pivot_lows.push(index);
        }
    }

    let bearish = if pivot_highs.len() >= 2 {
        let first = pivot_highs[pivot_highs.len() - 2];
        let second = pivot_highs[pivot_highs.len() - 1];
        match (rsi[first], rsi[second]) {
            (Some(first_rsi), Some(second_rsi)) => {
                second - first >= MIN_PIVOT_SEPARATION
                    && candles[second].high > candles[first].high
                    && second_rsi < first_rsi
            }
            _ => false,
        }
    } else {
        false
    };

    let bullish = if pivot_lows.len() >= 2 {
        let first = pivot_lows[pivot_lows.len() - 2];
        let second = pivot_lows[pivot_lows.len() - 1];
        match (rsi[first], rsi[second]) {
            (Some(first_rsi), Some(second_rsi)) => {
                second - first >= MIN_PIVOT_SEPARATION
                    && candles[second].low < candles[first].low
                    && second_rsi > first_rsi
            }
            _ => false,
        }
    } else {
        false
    };

    match (bearish, bullish) {
        (true, true) => {
            let bearish_pivot = pivot_highs[pivot_highs.len() - 1];
            let bullish_pivot = pivot_lows[pivot_lows.len() - 1];
            if bullish_pivot > bearish_pivot {
                RsiDivergence::Bullish
            } else {
                RsiDivergence::Bearish
            }
        }
        (true, false) => RsiDivergence::Bearish,
        (false, true) => RsiDivergence::Bullish,
        (false, false) => RsiDivergence::None,
    }
}

/// Compute the latest RSI value, its state, and the current divergence.
pub fn rsi_snapshot(
    candles: &[Candle],
    period: usize,
    overbought: f64,
    oversold: f64,
    lookback: usize,
    pivot_k: usize,
) -> RsiSnapshot {
    let series = rsi_series(candles, period);
    let value = series.last().copied().flatten();
    let state = value
        .map(|value| rsi_state(value, overbought, oversold))
        .unwrap_or_else(|| "Neutral".to_string());
    let divergence = detect_divergence(candles, &series, lookback, pivot_k);

    RsiSnapshot {
        value,
        state,
        divergence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;

    const STEP_MS: i64 = 15 * 60 * 1000;

    fn close_candle(index: usize, close: f64) -> Candle {
        Candle {
            timestamp: index as i64 * STEP_MS,
            open: close,
            high: close + 0.5,
            low: close - 0.5,
            close,
            volume: 100.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    fn candle_at(index: usize, high: f64, low: f64) -> Candle {
        let close = (high + low) / 2.0;
        Candle {
            timestamp: index as i64 * STEP_MS,
            open: close,
            high,
            low,
            close,
            volume: 100.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    fn pivot_candles(high_peaks: &[(usize, f64)], low_peaks: &[(usize, f64)]) -> Vec<Candle> {
        let mut candles: Vec<Candle> = (0..20).map(|index| candle_at(index, 100.0, 90.0)).collect();
        for &(index, high) in high_peaks {
            candles[index].high = high;
        }
        for &(index, low) in low_peaks {
            candles[index].low = low;
        }
        candles
    }

    #[test]
    fn rsi_series_matches_known_wilder_values() {
        // Seven +1 changes followed by seven -1 changes: the first RSI is 50.
        let mut closes = vec![100.0];
        let mut price = 100.0;
        for _ in 0..7 {
            price += 1.0;
            closes.push(price);
        }
        for _ in 0..7 {
            price -= 1.0;
            closes.push(price);
        }

        let mut candles: Vec<Candle> = closes
            .iter()
            .enumerate()
            .map(|(index, close)| close_candle(index, *close))
            .collect();
        let rsi = rsi_series(&candles, 14);

        assert_eq!(rsi.len(), candles.len());
        assert!(rsi[..14].iter().all(Option::is_none));
        assert!((rsi[14].expect("first RSI") - 50.0).abs() < 1e-9);

        // One more loss: avg gain = 13/28, avg loss = 15/28 -> RSI 46.4286.
        candles.push(close_candle(15, 99.0));
        let rsi = rsi_series(&candles, 14);
        let expected = 100.0 - 100.0 / (1.0 + (13.0 / 28.0) / (15.0 / 28.0));
        assert!((rsi[15].expect("second RSI") - expected).abs() < 1e-9);
    }

    #[test]
    fn rsi_series_handles_all_gain_all_loss_and_flat_series() {
        let all_gain: Vec<Candle> = (0..15)
            .map(|index| close_candle(index, 100.0 + index as f64))
            .collect();
        assert_eq!(rsi_series(&all_gain, 14)[14], Some(100.0));

        let all_loss: Vec<Candle> = (0..15)
            .map(|index| close_candle(index, 100.0 - index as f64))
            .collect();
        assert_eq!(rsi_series(&all_loss, 14)[14], Some(0.0));

        let flat: Vec<Candle> = (0..15).map(|index| close_candle(index, 100.0)).collect();
        assert_eq!(rsi_series(&flat, 14)[14], Some(50.0));

        // Not enough candles or a zero period leaves the whole series empty.
        assert!(rsi_series(&flat[..14], 14).iter().all(Option::is_none));
        assert!(rsi_series(&flat, 0).iter().all(Option::is_none));
    }

    #[test]
    fn detect_divergence_finds_bearish_higher_high_lower_rsi_high() {
        let candles = pivot_candles(&[(5, 110.0), (12, 115.0)], &[]);
        let mut rsi = vec![Some(50.0); candles.len()];
        rsi[5] = Some(70.0);
        rsi[12] = Some(60.0);

        assert_eq!(
            detect_divergence(&candles, &rsi, 20, 2),
            RsiDivergence::Bearish
        );
    }

    #[test]
    fn detect_divergence_finds_bullish_lower_low_higher_rsi_low() {
        let candles = pivot_candles(&[], &[(5, 80.0), (12, 75.0)]);
        let mut rsi = vec![Some(50.0); candles.len()];
        rsi[5] = Some(30.0);
        rsi[12] = Some(40.0);

        assert_eq!(
            detect_divergence(&candles, &rsi, 20, 2),
            RsiDivergence::Bullish
        );
    }

    #[test]
    fn detect_divergence_requires_price_and_rsi_disagreement() {
        // Price higher high and RSI higher high: no bearish divergence.
        let candles = pivot_candles(&[(5, 110.0), (12, 115.0)], &[]);
        let mut rsi = vec![Some(50.0); candles.len()];
        rsi[5] = Some(60.0);
        rsi[12] = Some(70.0);
        assert_eq!(
            detect_divergence(&candles, &rsi, 20, 2),
            RsiDivergence::None
        );

        // Pivots only two candles apart do not qualify.
        let close_pivots = pivot_candles(&[(5, 110.0), (7, 115.0)], &[]);
        let mut close_rsi = vec![Some(50.0); close_pivots.len()];
        close_rsi[5] = Some(70.0);
        close_rsi[7] = Some(60.0);
        assert_eq!(
            detect_divergence(&close_pivots, &close_rsi, 20, 2),
            RsiDivergence::None
        );

        // Missing RSI at a pivot also suppresses the signal.
        let mut missing_rsi = vec![Some(50.0); candles.len()];
        missing_rsi[5] = Some(70.0);
        missing_rsi[12] = None;
        assert_eq!(
            detect_divergence(&candles, &missing_rsi, 20, 2),
            RsiDivergence::None
        );
    }

    #[test]
    fn rsi_state_and_snapshot_report_thresholds_and_divergence() {
        assert_eq!(rsi_state(70.0, 70.0, 30.0), "Overbought");
        assert_eq!(rsi_state(69.9, 70.0, 30.0), "Neutral");
        assert_eq!(rsi_state(30.0, 70.0, 30.0), "Oversold");

        let candles: Vec<Candle> = (0..15)
            .map(|index| close_candle(index, 100.0 + index as f64))
            .collect();
        let snapshot = rsi_snapshot(&candles, 14, 70.0, 30.0, 20, 2);
        assert_eq!(snapshot.value, Some(100.0));
        assert_eq!(snapshot.state, "Overbought");
        assert_eq!(snapshot.divergence, RsiDivergence::None);
    }
}
