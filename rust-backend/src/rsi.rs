//! RSI (Wilder) and RSI/price divergence primitives.
//!
//! This module is deliberately side-effect free: it does not change live signal
//! generation by itself. The backtest binary (Phase R0) consumes it to evaluate
//! RSI extreme and divergence gates.

use crate::models::{BotConfig, Candle};
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

/// Nhãn divergence ổn định cho Signal/Telegram/API.
pub fn divergence_label(divergence: RsiDivergence) -> String {
    match divergence {
        RsiDivergence::None => "None",
        RsiDivergence::Bullish => "Bullish",
        RsiDivergence::Bearish => "Bearish",
    }
    .to_string()
}

/// Cổng lọc RSI tùy chọn cho tín hiệu thật.
///
/// Mặc định `rsi_filter_enabled=false` nên hàm luôn cho qua. Khi bật:
/// - `EXTREME`: BUY cần RSI <= oversold; SELL cần RSI >= overbought.
/// - `DIVERGENCE`: BUY cần Bullish divergence; SELL cần Bearish divergence.
/// - `BOTH`: một trong hai điều kiện trên.
///
/// Đây là filter cứng duy nhất của RSI; RSI vẫn luôn được hiển thị và gửi AI
/// như context ngay cả khi filter tắt.
pub fn rsi_allows_signal(
    signal_type: &str,
    snapshot: &RsiSnapshot,
    config: &BotConfig,
) -> (bool, String) {
    let signal = signal_type.trim().to_uppercase();
    if !matches!(signal.as_str(), "BUY" | "LONG" | "SELL" | "SHORT") {
        return (
            true,
            format!(
                "Loại tín hiệu '{}' không xác định — bỏ qua bộ lọc RSI.",
                signal
            ),
        );
    }

    if !config.rsi_filter_enabled {
        return (true, "RSI filter disabled".to_string());
    }

    let is_buy = matches!(signal.as_str(), "BUY" | "LONG");
    let value = snapshot
        .value
        .map(|value| format!("{:.1}", value))
        .unwrap_or_else(|| "n/a".to_string());
    let divergence = divergence_label(snapshot.divergence);
    let state = snapshot.state.as_str();

    let extreme_allows = match snapshot.value {
        Some(value) if is_buy => value <= config.rsi_oversold,
        Some(value) => value >= config.rsi_overbought,
        None => false,
    };
    let divergence_allows = if is_buy {
        snapshot.divergence == RsiDivergence::Bullish
    } else {
        snapshot.divergence == RsiDivergence::Bearish
    };
    let divergence_component = config.rsi_divergence_enabled && divergence_allows;

    let mode = config.rsi_mode.trim().to_uppercase();
    let (allowed, mode_label) = match mode.as_str() {
        "EXTREME" => (extreme_allows, "EXTREME"),
        "DIVERGENCE" => {
            if !config.rsi_divergence_enabled {
                return (
                    false,
                    format!(
                        "RSI DIVERGENCE chặn {}: divergence disabled (rsi_divergence_enabled=false) · value={} state={} divergence={}.",
                        signal, value, state, divergence
                    ),
                );
            }
            (divergence_allows, "DIVERGENCE")
        }
        _ => (extreme_allows || divergence_component, "BOTH"),
    };

    if allowed {
        (
            true,
            format!(
                "RSI {} cho qua {} · value={} state={} divergence={} (oversold<= {:.1}, overbought>= {:.1}).",
                mode_label, signal, value, state, divergence, config.rsi_oversold, config.rsi_overbought
            ),
        )
    } else {
        let requirement = if is_buy {
            format!(
                "BUY cần RSI <= {:.1} hoặc Bullish divergence",
                config.rsi_oversold
            )
        } else {
            format!(
                "SELL cần RSI >= {:.1} hoặc Bearish divergence",
                config.rsi_overbought
            )
        };
        (
            false,
            format!(
                "RSI {} chặn {} · value={} state={} divergence={} — {}.",
                mode_label, signal, value, state, divergence, requirement
            ),
        )
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

    fn snapshot(value: Option<f64>, state: &str, divergence: RsiDivergence) -> RsiSnapshot {
        RsiSnapshot {
            value,
            state: state.to_string(),
            divergence,
        }
    }

    fn rsi_config(mode: &str) -> BotConfig {
        BotConfig {
            rsi_filter_enabled: true,
            rsi_mode: mode.to_string(),
            ..BotConfig::default()
        }
    }

    #[test]
    fn rsi_filter_disabled_allows_every_signal() {
        let config = BotConfig::default();
        let neutral = snapshot(Some(50.0), "Neutral", RsiDivergence::None);
        let (allowed, reason) = rsi_allows_signal("BUY", &neutral, &config);
        assert!(allowed);
        assert!(reason.contains("disabled"));
        assert!(rsi_allows_signal("SELL", &neutral, &config).0);
    }

    #[test]
    fn rsi_extreme_mode_requires_oversold_buy_and_overbought_sell() {
        let config = rsi_config("EXTREME");
        let oversold = snapshot(Some(25.0), "Oversold", RsiDivergence::None);
        let overbought = snapshot(Some(78.0), "Overbought", RsiDivergence::None);
        let neutral = snapshot(Some(50.0), "Neutral", RsiDivergence::None);

        let (allowed, reason) = rsi_allows_signal("BUY", &oversold, &config);
        assert!(allowed, "{}", reason);
        assert!(reason.contains("value=25.0"));
        assert!(reason.contains("Oversold"));

        let (allowed, reason) = rsi_allows_signal("SELL", &overbought, &config);
        assert!(allowed, "{}", reason);

        let (allowed, reason) = rsi_allows_signal("BUY", &neutral, &config);
        assert!(!allowed);
        assert!(reason.contains("Neutral"));
        assert!(reason.contains("divergence=None"));

        assert!(!rsi_allows_signal("SELL", &oversold, &config).0);
        assert!(!rsi_allows_signal("BUY", &overbought, &config).0);
    }

    #[test]
    fn rsi_divergence_mode_requires_matching_divergence() {
        let config = rsi_config("DIVERGENCE");
        let bullish = snapshot(Some(45.0), "Neutral", RsiDivergence::Bullish);
        let bearish = snapshot(Some(55.0), "Neutral", RsiDivergence::Bearish);
        let none = snapshot(Some(55.0), "Neutral", RsiDivergence::None);

        let (allowed, reason) = rsi_allows_signal("BUY", &bullish, &config);
        assert!(allowed, "{}", reason);
        assert!(reason.contains("divergence=Bullish"));

        let (allowed, reason) = rsi_allows_signal("SELL", &bearish, &config);
        assert!(allowed, "{}", reason);
        assert!(reason.contains("divergence=Bearish"));

        assert!(!rsi_allows_signal("BUY", &bearish, &config).0);
        assert!(!rsi_allows_signal("SELL", &bullish, &config).0);
        assert!(!rsi_allows_signal("BUY", &none, &config).0);
    }

    #[test]
    fn rsi_divergence_mode_blocks_when_divergence_component_disabled() {
        let config = BotConfig {
            rsi_divergence_enabled: false,
            ..rsi_config("DIVERGENCE")
        };
        let bullish = snapshot(Some(45.0), "Neutral", RsiDivergence::Bullish);
        let (allowed, reason) = rsi_allows_signal("BUY", &bullish, &config);
        assert!(!allowed);
        assert!(reason.contains("divergence disabled"));
    }

    #[test]
    fn rsi_both_mode_is_or_of_extreme_and_divergence() {
        let config = rsi_config("BOTH");
        let neutral_bullish = snapshot(Some(50.0), "Neutral", RsiDivergence::Bullish);
        let neutral_bearish = snapshot(Some(50.0), "Neutral", RsiDivergence::Bearish);
        let overbought_none = snapshot(Some(80.0), "Overbought", RsiDivergence::None);
        let neutral_none = snapshot(Some(50.0), "Neutral", RsiDivergence::None);

        assert!(rsi_allows_signal("BUY", &neutral_bullish, &config).0);
        assert!(rsi_allows_signal("SELL", &neutral_bearish, &config).0);
        assert!(rsi_allows_signal("SELL", &overbought_none, &config).0);
        assert!(!rsi_allows_signal("BUY", &overbought_none, &config).0);
        assert!(!rsi_allows_signal("BUY", &neutral_none, &config).0);
        assert!(!rsi_allows_signal("SELL", &neutral_none, &config).0);
    }

    #[test]
    fn rsi_none_value_blocks_extreme_and_both_without_divergence() {
        let none = snapshot(None, "Neutral", RsiDivergence::None);
        for mode in ["EXTREME", "BOTH"] {
            let config = rsi_config(mode);
            let (allowed, reason) = rsi_allows_signal("BUY", &none, &config);
            assert!(!allowed, "mode {} must block without a value", mode);
            assert!(reason.contains("value=n/a"));
        }
    }

    #[test]
    fn rsi_custom_65_35_thresholds_are_respected() {
        let config = BotConfig {
            rsi_overbought: 65.0,
            rsi_oversold: 35.0,
            ..rsi_config("EXTREME")
        };

        assert!(
            rsi_allows_signal(
                "SELL",
                &snapshot(Some(65.0), "Overbought", RsiDivergence::None),
                &config
            )
            .0
        );
        assert!(
            !rsi_allows_signal(
                "SELL",
                &snapshot(Some(64.9), "Neutral", RsiDivergence::None),
                &config
            )
            .0
        );
        assert!(
            rsi_allows_signal(
                "BUY",
                &snapshot(Some(35.0), "Oversold", RsiDivergence::None),
                &config
            )
            .0
        );
        assert!(
            !rsi_allows_signal(
                "BUY",
                &snapshot(Some(35.1), "Neutral", RsiDivergence::None),
                &config
            )
            .0
        );
    }

    #[test]
    fn rsi_filter_allows_unknown_signal_types() {
        let config = rsi_config("EXTREME");
        let neutral = snapshot(Some(50.0), "Neutral", RsiDivergence::None);
        let (allowed, reason) = rsi_allows_signal("UNKNOWN", &neutral, &config);
        assert!(allowed);
        assert!(reason.contains("không xác định"));
    }

    #[test]
    fn divergence_label_matches_serialized_direction() {
        assert_eq!(divergence_label(RsiDivergence::None), "None");
        assert_eq!(divergence_label(RsiDivergence::Bullish), "Bullish");
        assert_eq!(divergence_label(RsiDivergence::Bearish), "Bearish");
    }
}
