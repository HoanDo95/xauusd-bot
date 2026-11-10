use crate::models::{BotConfig, Candle};
use serde::{Deserialize, Serialize};

/// Ảnh chụp dòng tiền mua/bán chủ động (taker flow) của cây nến tín hiệu
/// và CVD tích lũy trong cửa sổ lookback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaSnapshot {
    pub buy_volume: f64,
    pub sell_volume: f64,
    pub delta: f64,
    pub delta_ratio: f64,
    pub cvd: f64,
    pub cvd_change: f64,
}

fn candle_delta(candle: &Candle) -> f64 {
    let buy = candle.taker_buy_volume.min(candle.volume);
    let sell = candle.volume - buy;
    buy - sell
}

fn window_start(len: usize, lookback: usize) -> usize {
    len.saturating_sub(lookback.max(1))
}

/// Tính delta của cây nến mới nhất và CVD trong `lookback` nến gần nhất.
///
/// - `buy_volume = min(taker_buy_volume, volume)`.
/// - `sell_volume = volume - buy_volume`.
/// - `delta = buy_volume - sell_volume`.
/// - `delta_ratio = delta / volume` (0 khi volume = 0).
/// - `cvd` = tổng delta của cửa sổ lookback.
/// - `cvd_change` = CVD cuối − CVD đầu cửa sổ (tổng delta sau cây đầu tiên).
pub fn compute_delta(candles: &[Candle], lookback: usize) -> DeltaSnapshot {
    let Some(latest) = candles.last() else {
        return DeltaSnapshot {
            buy_volume: 0.0,
            sell_volume: 0.0,
            delta: 0.0,
            delta_ratio: 0.0,
            cvd: 0.0,
            cvd_change: 0.0,
        };
    };

    let buy_volume = latest.taker_buy_volume.min(latest.volume);
    let sell_volume = latest.volume - buy_volume;
    let delta = buy_volume - sell_volume;
    let delta_ratio = if latest.volume > 0.0 {
        delta / latest.volume
    } else {
        0.0
    };

    let start = window_start(candles.len(), lookback);
    let window = &candles[start..];
    let mut cvd = 0.0;
    let mut first_delta = 0.0;
    for (index, candle) in window.iter().enumerate() {
        let candle_flow = candle_delta(candle);
        if index == 0 {
            first_delta = candle_flow;
        }
        cvd += candle_flow;
    }
    let cvd_change = if window.len() > 1 {
        cvd - first_delta
    } else {
        0.0
    };

    DeltaSnapshot {
        buy_volume,
        sell_volume,
        delta,
        delta_ratio,
        cvd,
        cvd_change,
    }
}

fn lookback_price_change(candles: &[Candle], lookback: usize) -> f64 {
    let Some(last) = candles.last() else {
        return 0.0;
    };
    if candles.len() < 2 {
        return 0.0;
    }

    let start = window_start(candles.len(), lookback);
    last.close - candles[start].close
}

fn mode_uppercase(mode: &str) -> String {
    mode.trim().to_uppercase()
}

/// Bộ lọc Volume Delta/CVD tùy chọn. Mặc định TẮT => luôn cho qua.
///
/// - `ALIGN`: BUY cần `delta_ratio >= min_ratio`; SELL cần `<= -min_ratio`.
/// - `DIVERGENCE`: BUY khi giá giảm nhưng CVD tăng; SELL khi giá tăng nhưng CVD giảm.
/// - `BOTH`: ALIGN hoặc DIVERGENCE.
pub fn delta_allows_signal(
    signal_type: &str,
    candles: &[Candle],
    config: &BotConfig,
) -> (bool, String, DeltaSnapshot) {
    let snapshot = compute_delta(candles, config.volume_delta_lookback);

    if !config.volume_delta_filter_enabled {
        return (true, "Volume Delta filter disabled".to_string(), snapshot);
    }

    let signal = signal_type.trim().to_uppercase();
    let min_ratio = config.volume_delta_min_ratio.clamp(0.0, 1.0);
    let price_change = lookback_price_change(candles, config.volume_delta_lookback);

    let align_allowed = match signal.as_str() {
        "BUY" | "LONG" => snapshot.delta_ratio >= min_ratio,
        "SELL" | "SHORT" => snapshot.delta_ratio <= -min_ratio,
        _ => true,
    };
    let divergence_allowed = match signal.as_str() {
        "BUY" | "LONG" => price_change < 0.0 && snapshot.cvd_change > 0.0,
        "SELL" | "SHORT" => price_change > 0.0 && snapshot.cvd_change < 0.0,
        _ => false,
    };

    let mode = mode_uppercase(&config.volume_delta_mode);
    let allowed = match mode.as_str() {
        "ALIGN" => align_allowed,
        "DIVERGENCE" => divergence_allowed,
        _ => align_allowed || divergence_allowed,
    };

    let mode_label = if mode.is_empty() {
        "BOTH"
    } else {
        mode.as_str()
    };
    let status = if allowed { "cho qua" } else { "chặn" };
    let reason = format!(
        "Volume Delta {} {} (mode={}, delta_ratio={:+.3}, ngưỡng={:+.3}, price_change={:+.2}, cvd_change={:+.2})",
        status, signal, mode_label, snapshot.delta_ratio, min_ratio, price_change, snapshot.cvd_change
    );

    (allowed, reason, snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;

    fn candle(close: f64, volume: f64, taker_buy_volume: f64) -> Candle {
        Candle {
            timestamp: 1_700_000_000_000 + (close * 1000.0) as i64,
            open: close,
            high: close + 1.0,
            low: close - 1.0,
            close,
            volume,
            taker_buy_volume,
            timeframe: Timeframe::M15,
        }
    }

    fn config(mode: &str) -> BotConfig {
        BotConfig {
            volume_delta_filter_enabled: true,
            volume_delta_mode: mode.to_string(),
            volume_delta_lookback: 5,
            volume_delta_min_ratio: 0.10,
            ..BotConfig::default()
        }
    }

    #[test]
    fn compute_delta_uses_latest_candle_and_window() {
        let candles = vec![candle(100.0, 100.0, 70.0), candle(101.0, 100.0, 20.0)];
        let snapshot = compute_delta(&candles, 20);

        assert!((snapshot.buy_volume - 20.0).abs() < 1e-9);
        assert!((snapshot.sell_volume - 80.0).abs() < 1e-9);
        assert!((snapshot.delta + 60.0).abs() < 1e-9);
        assert!((snapshot.delta_ratio + 0.6).abs() < 1e-9);
        assert!((snapshot.cvd + 20.0).abs() < 1e-9);
        assert!((snapshot.cvd_change + 60.0).abs() < 1e-9);
    }

    #[test]
    fn compute_delta_handles_empty_and_zero_volume() {
        let empty = compute_delta(&[], 20);
        assert_eq!(empty.delta, 0.0);
        assert_eq!(empty.delta_ratio, 0.0);
        assert_eq!(empty.cvd, 0.0);

        let zero = compute_delta(&[candle(100.0, 0.0, 5.0)], 20);
        assert_eq!(zero.buy_volume, 0.0);
        assert_eq!(zero.sell_volume, 0.0);
        assert_eq!(zero.delta, 0.0);
        assert_eq!(zero.delta_ratio, 0.0);
        assert_eq!(zero.cvd, 0.0);
        assert_eq!(zero.cvd_change, 0.0);
    }

    #[test]
    fn align_mode_requires_flow_on_trade_side() {
        let buy_flow = vec![candle(100.0, 100.0, 70.0)];
        let sell_flow = vec![candle(100.0, 100.0, 30.0)];
        let align = config("ALIGN");

        let (allowed, reason, snapshot) = delta_allows_signal("BUY", &buy_flow, &align);
        assert!(allowed, "{}", reason);
        assert!((snapshot.delta_ratio - 0.4).abs() < 1e-9);

        let (allowed, reason, _) = delta_allows_signal("SELL", &sell_flow, &align);
        assert!(allowed, "{}", reason);

        assert!(!delta_allows_signal("BUY", &sell_flow, &align).0);
        assert!(!delta_allows_signal("SELL", &buy_flow, &align).0);
    }

    #[test]
    fn divergence_mode_detects_bullish_and_bearish_absorption() {
        let bullish: Vec<Candle> = [100.0, 99.0, 98.0, 97.0, 96.0]
            .into_iter()
            .map(|close| candle(close, 100.0, 70.0))
            .collect();
        let bearish: Vec<Candle> = [100.0, 101.0, 102.0, 103.0, 104.0]
            .into_iter()
            .map(|close| candle(close, 100.0, 30.0))
            .collect();
        let divergence = config("DIVERGENCE");

        let (allowed, reason, snapshot) = delta_allows_signal("BUY", &bullish, &divergence);
        assert!(allowed, "{}", reason);
        assert!(snapshot.cvd_change > 0.0);
        assert!(!delta_allows_signal("SELL", &bullish, &divergence).0);

        let (allowed, reason, snapshot) = delta_allows_signal("SELL", &bearish, &divergence);
        assert!(allowed, "{}", reason);
        assert!(snapshot.cvd_change < 0.0);
        assert!(!delta_allows_signal("BUY", &bearish, &divergence).0);
    }

    #[test]
    fn both_mode_is_or_between_align_and_divergence() {
        let both = config("BOTH");

        // ALIGN-only: cây nến duy nhất có delta dương, price_change = 0.
        let align_only = vec![candle(100.0, 100.0, 70.0)];
        assert!(delta_allows_signal("BUY", &align_only, &both).0);

        // DIVERGENCE-only: giá giảm, CVD tăng nhưng delta cây cuối âm.
        let divergence_only: Vec<Candle> = [100.0, 99.0, 98.0, 97.0]
            .into_iter()
            .map(|close| candle(close, 100.0, 70.0))
            .chain(std::iter::once(candle(96.0, 100.0, 30.0)))
            .collect();
        let (allowed, reason, snapshot) = delta_allows_signal("BUY", &divergence_only, &both);
        assert!(allowed, "{}", reason);
        assert!(snapshot.delta_ratio < 0.0);

        // Cả hai vế đều fail: giá giảm + CVD giảm + delta cây cuối âm.
        let neither: Vec<Candle> = [100.0, 99.0, 98.0, 97.0, 96.0]
            .into_iter()
            .map(|close| candle(close, 100.0, 30.0))
            .collect();
        assert!(!delta_allows_signal("BUY", &neither, &both).0);
    }

    #[test]
    fn disabled_filter_always_allows_and_still_returns_snapshot() {
        let candles = vec![candle(100.0, 100.0, 10.0)];
        let disabled = BotConfig {
            volume_delta_filter_enabled: false,
            volume_delta_mode: "ALIGN".to_string(),
            ..BotConfig::default()
        };

        let (allowed, reason, snapshot) = delta_allows_signal("BUY", &candles, &disabled);
        assert!(allowed);
        assert!(reason.contains("disabled"));
        assert!(snapshot.delta_ratio < 0.0);
    }
}
