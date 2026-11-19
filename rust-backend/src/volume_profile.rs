//! Volume profile (POC / value area) from closed candles.
//!
//! Context only: it is displayed on the scalp dashboard and attached to the
//! research alert. It never changes an alert decision.

use crate::models::Candle;
use serde::Serialize;

pub const DEFAULT_LOOKBACK: usize = 96;
pub const DEFAULT_BINS: usize = 50;
pub const DEFAULT_VALUE_AREA: f64 = 0.70;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VolumeProfile {
    pub poc: f64,
    pub val: f64,
    pub vah: f64,
    pub window_low: f64,
    pub window_high: f64,
    pub bars: usize,
}

/// Build a volume profile over the last `lookback` candles.
///
/// Each candle's tick volume is distributed uniformly across its high-low
/// range. The value area grows from the POC until it covers `value_area` of
/// the total volume.
pub fn compute(
    candles: &[Candle],
    lookback: usize,
    bins: usize,
    value_area: f64,
) -> Option<VolumeProfile> {
    if bins < 3 || candles.len() < 10 {
        return None;
    }
    let window = &candles[candles.len().saturating_sub(lookback.max(1))..];
    let window_low = window
        .iter()
        .map(|candle| candle.low)
        .fold(f64::INFINITY, f64::min);
    let window_high = window
        .iter()
        .map(|candle| candle.high)
        .fold(f64::NEG_INFINITY, f64::max);
    if !window_low.is_finite() || !window_high.is_finite() || window_high <= window_low {
        return None;
    }
    let width = (window_high - window_low) / bins as f64;
    if width <= 0.0 {
        return None;
    }

    let mut buckets = vec![0.0_f64; bins];
    for candle in window {
        if !candle.volume.is_finite() || candle.volume <= 0.0 {
            continue;
        }
        if candle.high <= candle.low {
            let idx = (((candle.low - window_low) / width).floor() as isize)
                .clamp(0, bins as isize - 1) as usize;
            buckets[idx] += candle.volume;
            continue;
        }
        let span = candle.high - candle.low;
        let first = (((candle.low - window_low) / width).floor() as isize)
            .clamp(0, bins as isize - 1) as usize;
        let last = (((candle.high - window_low) / width).floor() as isize)
            .clamp(0, bins as isize - 1) as usize;
        for (index, bucket) in buckets.iter_mut().enumerate().take(last + 1).skip(first) {
            let bin_low = window_low + index as f64 * width;
            let bin_high = bin_low + width;
            let overlap = candle.high.min(bin_high) - candle.low.max(bin_low);
            if overlap > 0.0 {
                *bucket += candle.volume * overlap / span;
            }
        }
    }

    let total: f64 = buckets.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut poc_idx = 0;
    for index in 1..bins {
        if buckets[index] > buckets[poc_idx] {
            poc_idx = index;
        }
    }

    let mut low_idx = poc_idx;
    let mut high_idx = poc_idx;
    let mut covered = buckets[poc_idx];
    let target = total * value_area.clamp(0.1, 0.99);
    while covered < target && (low_idx > 0 || high_idx + 1 < bins) {
        let left = if low_idx > 0 {
            buckets[low_idx - 1]
        } else {
            -1.0
        };
        let right = if high_idx + 1 < bins {
            buckets[high_idx + 1]
        } else {
            -1.0
        };
        if right >= left {
            high_idx += 1;
            covered += buckets[high_idx];
        } else {
            low_idx -= 1;
            covered += buckets[low_idx];
        }
    }

    Some(VolumeProfile {
        poc: window_low + (poc_idx as f64 + 0.5) * width,
        val: window_low + low_idx as f64 * width,
        vah: window_low + (high_idx as f64 + 1.0) * width,
        window_low,
        window_high,
        bars: window.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;

    fn candle(low: f64, high: f64, close: f64, volume: f64) -> Candle {
        Candle {
            timestamp: 0,
            open: close,
            high,
            low,
            close,
            volume,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    #[test]
    fn poc_follows_the_high_volume_cluster() {
        let mut candles = Vec::new();
        for _ in 0..40 {
            candles.push(candle(100.0, 101.0, 100.5, 10.0));
        }
        for _ in 0..40 {
            candles.push(candle(109.0, 110.0, 109.5, 100.0));
        }
        for _ in 0..20 {
            candles.push(candle(104.0, 106.0, 105.0, 5.0));
        }
        let profile = compute(&candles, 96, 50, 0.70).expect("profile");
        assert!((109.0..=110.0).contains(&profile.poc));
        assert!(profile.val <= profile.poc);
        assert!(profile.poc <= profile.vah);
        assert!(profile.val >= 100.0);
        assert!(profile.vah <= 110.0);
    }

    #[test]
    fn zero_volume_returns_none() {
        let candles: Vec<Candle> = (0..60)
            .map(|index| {
                candle(
                    100.0 + index as f64 * 0.1,
                    100.2 + index as f64 * 0.1,
                    100.1,
                    0.0,
                )
            })
            .collect();
        assert!(compute(&candles, 96, 50, 0.70).is_none());
    }

    #[test]
    fn flat_window_returns_none() {
        let candles: Vec<Candle> = (0..60).map(|_| candle(100.0, 100.0, 100.0, 10.0)).collect();
        assert!(compute(&candles, 96, 50, 0.70).is_none());
    }
}
