use crate::analyzer::GoldMarketAnalyzer;
use crate::models::Candle;
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Số nến dùng cho kênh Donchian và ATR.
const CHANNEL_PERIOD: usize = 20;
const ATR_PERIOD: usize = 14;
/// Số nến tối đa dùng để dò hỗ trợ / kháng cự động.
const SR_LOOKBACK: usize = 100;
/// Số nến cuối dùng cho fallback swing high/low khi không có zone.
const SWING_FALLBACK_LOOKBACK: usize = 20;

/// Bối cảnh kỹ thuật của một khung thời gian lớn (D1/H4/H1).
/// Read-only/advisory: chỉ dùng để hiển thị "view toàn cảnh" và làm bộ lọc tùy chọn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeframeContext {
    /// "D1" | "H4" | "H1"
    pub timeframe: String,
    /// "BULLISH" | "BEARISH" | "NEUTRAL"
    pub bias: String,
    pub close: f64,
    pub ema50: f64,
    pub ema200: f64,
    /// Vị trí trong kênh Donchian(20), 0..=1 (0 = sát đáy kênh, 1 = sát đỉnh kênh).
    pub channel_position: f64,
    pub atr14: f64,
    pub nearest_support: Option<f64>,
    pub nearest_resistance: Option<f64>,
    pub candles: usize,
}

/// Toàn bộ bối cảnh đa khung D1/H4/H1 tại một thời điểm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketContext {
    pub d1: TimeframeContext,
    pub h4: TimeframeContext,
    pub h1: TimeframeContext,
    pub updated_at: String,
    /// Ví dụ "BINANCE_PAXG" hoặc "BINANCE_PAXG+TWELVEDATA_SPOT".
    pub price_source: String,
    /// Basis PAXG → spot XAU/USD đang áp dụng (nếu có).
    pub spot_basis: Option<f64>,
}

/// Dựng bối cảnh từ nến đã đóng của D1/H4/H1.
pub fn build_market_context(
    d1: &[Candle],
    h4: &[Candle],
    h1: &[Candle],
    price_source: &str,
    spot_basis: Option<f64>,
) -> MarketContext {
    MarketContext {
        d1: build_timeframe_context("D1", d1),
        h4: build_timeframe_context("H4", h4),
        h1: build_timeframe_context("H1", h1),
        updated_at: Utc::now().to_rfc3339(),
        price_source: price_source.to_string(),
        spot_basis,
    }
}

/// Dựng bối cảnh cho một khung thời gian. Chuỗi rỗng trả về context an toàn.
pub fn build_timeframe_context(timeframe: &str, candles: &[Candle]) -> TimeframeContext {
    let timeframe = timeframe.trim().to_uppercase();

    if candles.is_empty() {
        return TimeframeContext {
            timeframe,
            bias: "NEUTRAL".to_string(),
            close: 0.0,
            ema50: 0.0,
            ema200: 0.0,
            channel_position: 0.5,
            atr14: 0.0,
            nearest_support: None,
            nearest_resistance: None,
            candles: 0,
        };
    }

    let close = candles.last().map(|candle| candle.close).unwrap_or(0.0);
    let ema50 = GoldMarketAnalyzer::calculate_ema(candles, 50);
    let ema200 = GoldMarketAnalyzer::calculate_ema(candles, 200);
    let bias = if close > ema50 && ema50 > ema200 {
        "BULLISH"
    } else if close < ema50 && ema50 < ema200 {
        "BEARISH"
    } else {
        "NEUTRAL"
    };
    let tolerance = sr_tolerance(&timeframe);
    let (nearest_support, nearest_resistance) = nearest_levels(candles, close, tolerance);

    TimeframeContext {
        timeframe,
        bias: bias.to_string(),
        close: round2(close),
        ema50: round2(ema50),
        ema200: round2(ema200),
        channel_position: channel_position(candles, close),
        atr14: round2(atr14(candles)),
        nearest_support,
        nearest_resistance,
        candles: candles.len(),
    }
}

/// Bộ lọc tùy chọn: chỉ chặn lệnh ngược H4.
/// - BUY bị chặn khi H4 BEARISH.
/// - SELL bị chặn khi H4 BULLISH.
/// - H4 NEUTRAL, D1 và H1 chỉ mang tính tham khảo.
pub fn h4_allows_signal(signal_type: &str, context: &MarketContext) -> (bool, String) {
    let signal = signal_type.trim().to_uppercase();
    let h4_bias = context.h4.bias.trim().to_uppercase();

    match signal.as_str() {
        "BUY" | "LONG" => {
            if h4_bias == "BEARISH" {
                (
                    false,
                    format!(
                        "H4 đang BEARISH (EMA50 {:.2} < EMA200 {:.2}) — chặn BUY ngược xu hướng H4.",
                        context.h4.ema50, context.h4.ema200
                    ),
                )
            } else {
                (
                    true,
                    format!("H4 {} — BUY không ngược xu hướng H4.", h4_bias),
                )
            }
        }
        "SELL" | "SHORT" => {
            if h4_bias == "BULLISH" {
                (
                    false,
                    format!(
                        "H4 đang BULLISH (EMA50 {:.2} > EMA200 {:.2}) — chặn SELL ngược xu hướng H4.",
                        context.h4.ema50, context.h4.ema200
                    ),
                )
            } else {
                (
                    true,
                    format!("H4 {} — SELL không ngược xu hướng H4.", h4_bias),
                )
            }
        }
        _ => (
            true,
            format!(
                "Loại tín hiệu '{}' không xác định — bỏ qua bộ lọc H4.",
                signal
            ),
        ),
    }
}

/// Bộ lọc tổng hợp H4 + D1 tùy chọn. Mặc định cả hai tắt => luôn cho qua.
pub fn htf_allows_signal(
    signal_type: &str,
    context: &MarketContext,
    h4_enabled: bool,
    d1_enabled: bool,
) -> (bool, String) {
    if !h4_enabled && !d1_enabled {
        return (true, "HTF filter disabled".to_string());
    }

    if h4_enabled {
        let (allowed, reason) = h4_allows_signal(signal_type, context);
        if !allowed {
            return (false, reason);
        }
    }

    if d1_enabled {
        let signal = signal_type.trim().to_uppercase();
        let d1_bias = context.d1.bias.trim().to_uppercase();
        match signal.as_str() {
            "BUY" | "LONG" if d1_bias == "BEARISH" => {
                return (
                    false,
                    format!(
                        "D1 đang BEARISH (EMA50 {:.2} < EMA200 {:.2}) — chặn BUY ngược xu hướng D1.",
                        context.d1.ema50, context.d1.ema200
                    ),
                );
            }
            "SELL" | "SHORT" if d1_bias == "BULLISH" => {
                return (
                    false,
                    format!(
                        "D1 đang BULLISH (EMA50 {:.2} > EMA200 {:.2}) — chặn SELL ngược xu hướng D1.",
                        context.d1.ema50, context.d1.ema200
                    ),
                );
            }
            _ => {}
        }
    }

    (true, "HTF filter passed".to_string())
}

/// Dịch toàn bộ OHLC của nến theo basis spot (PAXG → XAU/USD), làm tròn 2 chữ số.
/// Basis không hữu hạn hoặc bằng 0 là no-op.
pub fn shift_candles_prices(candles: &mut [Candle], basis: f64) {
    if !basis.is_finite() || basis == 0.0 {
        return;
    }

    for candle in candles {
        candle.open = round2(candle.open + basis);
        candle.high = round2(candle.high + basis);
        candle.low = round2(candle.low + basis);
        candle.close = round2(candle.close + basis);
    }
}

fn sr_tolerance(timeframe: &str) -> f64 {
    match timeframe {
        "D1" => 15.0,
        "H4" => 10.0,
        "H1" => 5.0,
        _ => 5.0,
    }
}

/// Vị trí giá đóng cửa trong kênh Donchian(20), clamp 0..=1.
/// Kênh phẳng (high == low) trả về 0.5 (neutral).
fn channel_position(candles: &[Candle], close: f64) -> f64 {
    let start = candles.len().saturating_sub(CHANNEL_PERIOD);
    let slice = &candles[start..];
    let high = slice
        .iter()
        .map(|candle| candle.high)
        .fold(f64::NEG_INFINITY, f64::max);
    let low = slice
        .iter()
        .map(|candle| candle.low)
        .fold(f64::INFINITY, f64::min);
    let range = high - low;

    if !range.is_finite() || range <= 0.0 {
        0.5
    } else {
        ((close - low) / range).clamp(0.0, 1.0)
    }
}

/// ATR(14) trên các cây nến đã đóng. Chuỗi ngắn dùng số nến hiện có.
fn atr14(candles: &[Candle]) -> f64 {
    if candles.is_empty() {
        return 0.0;
    }

    let start = candles.len().saturating_sub(ATR_PERIOD);
    let mut sum = 0.0;
    let mut count = 0usize;

    for index in start..candles.len() {
        let candle = &candles[index];
        let previous_close = if index > 0 {
            candles[index - 1].close
        } else {
            candle.close
        };
        let true_range = (candle.high - candle.low)
            .max((candle.high - previous_close).abs())
            .max((candle.low - previous_close).abs());

        if true_range.is_finite() {
            sum += true_range;
            count += 1;
        }
    }

    if count == 0 {
        0.0
    } else {
        sum / count as f64
    }
}

/// Hỗ trợ gần nhất dưới giá và kháng cự gần nhất trên giá.
/// Ưu tiên zone động từ pivot; fallback swing low/high của 20 nến cuối.
fn nearest_levels(candles: &[Candle], close: f64, tolerance: f64) -> (Option<f64>, Option<f64>) {
    let lookback = candles.len().min(SR_LOOKBACK);
    let mut support = None;
    let mut resistance = None;

    if lookback >= 5 {
        let slice = &candles[candles.len() - lookback..];
        let (supports, resistances) =
            GoldMarketAnalyzer::detect_dynamic_sr_zones(slice, lookback, tolerance);

        support = supports
            .iter()
            .map(|zone| zone.price)
            .filter(|price| *price < close)
            .max_by(f64::total_cmp);
        resistance = resistances
            .iter()
            .map(|zone| zone.price)
            .filter(|price| *price > close)
            .min_by(f64::total_cmp);
    }

    if support.is_none() || resistance.is_none() {
        let start = candles.len().saturating_sub(SWING_FALLBACK_LOOKBACK);
        let tail = &candles[start..];
        if support.is_none() {
            support = tail
                .iter()
                .map(|candle| candle.low)
                .filter(|price| *price < close)
                .max_by(f64::total_cmp);
        }
        if resistance.is_none() {
            resistance = tail
                .iter()
                .map(|candle| candle.high)
                .filter(|price| *price > close)
                .min_by(f64::total_cmp);
        }
    }

    (support.map(round2), resistance.map(round2))
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;

    const BASE_TS: i64 = 1_700_000_000_000;
    const STEP_MS: i64 = 15 * 60 * 1000;

    fn candle(timestamp: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume: 1000.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::H4,
        }
    }

    fn trend_candles(count: usize, start: f64, step: f64) -> Vec<Candle> {
        (0..count)
            .map(|index| {
                let close = start + index as f64 * step;
                candle(
                    BASE_TS + index as i64 * STEP_MS,
                    close - step / 2.0,
                    close + 1.0,
                    close - 1.0,
                    close,
                )
            })
            .collect()
    }

    fn timeframe_context(bias: &str) -> TimeframeContext {
        TimeframeContext {
            timeframe: "H4".to_string(),
            bias: bias.to_string(),
            close: 2400.0,
            ema50: 2380.0,
            ema200: 2360.0,
            channel_position: 0.5,
            atr14: 10.0,
            nearest_support: Some(2380.0),
            nearest_resistance: Some(2420.0),
            candles: 200,
        }
    }

    fn context_with_h4_bias(bias: &str) -> MarketContext {
        MarketContext {
            d1: timeframe_context("NEUTRAL"),
            h4: timeframe_context(bias),
            h1: timeframe_context("NEUTRAL"),
            updated_at: Utc::now().to_rfc3339(),
            price_source: "BINANCE_PAXG".to_string(),
            spot_basis: None,
        }
    }

    #[test]
    fn empty_candles_returns_neutral_safe_context() {
        let context = build_timeframe_context("D1", &[]);

        assert_eq!(context.timeframe, "D1");
        assert_eq!(context.bias, "NEUTRAL");
        assert_eq!(context.close, 0.0);
        assert_eq!(context.ema50, 0.0);
        assert_eq!(context.ema200, 0.0);
        assert_eq!(context.channel_position, 0.5);
        assert_eq!(context.atr14, 0.0);
        assert_eq!(context.nearest_support, None);
        assert_eq!(context.nearest_resistance, None);
        assert_eq!(context.candles, 0);
    }

    #[test]
    fn bullish_trend_detects_bullish_bias() {
        let candles = trend_candles(220, 2000.0, 1.0);
        let context = build_timeframe_context("H4", &candles);

        assert_eq!(context.bias, "BULLISH");
        assert!(context.ema50 > context.ema200);
        assert!(context.close > context.ema50);
        assert_eq!(context.candles, 220);
    }

    #[test]
    fn bearish_trend_detects_bearish_bias() {
        let candles = trend_candles(220, 2400.0, -1.0);
        let context = build_timeframe_context("D1", &candles);

        assert_eq!(context.bias, "BEARISH");
        assert!(context.ema50 < context.ema200);
        assert!(context.close < context.ema50);
    }

    #[test]
    fn channel_position_uses_donchian_20_and_clamps() {
        let mut candles: Vec<Candle> = (0..20)
            .map(|index| candle(BASE_TS + index * STEP_MS, 100.0, 100.0, 100.0, 100.0))
            .collect();
        candles[0].low = 90.0;
        candles[19].high = 110.0;
        candles[19].close = 100.0;

        let context = build_timeframe_context("H1", &candles);
        assert!((context.channel_position - 0.5).abs() < 1e-9);

        candles[19].close = 120.0;
        let context = build_timeframe_context("H1", &candles);
        assert_eq!(context.channel_position, 1.0);

        candles[19].close = 80.0;
        let context = build_timeframe_context("H1", &candles);
        assert_eq!(context.channel_position, 0.0);
    }

    #[test]
    fn atr14_averages_true_range() {
        // Mỗi nến rộng 10 và close trùng nhau -> TR = 10 cho mọi nến.
        let candles: Vec<Candle> = (0..14)
            .map(|index| candle(BASE_TS + index * STEP_MS, 100.0, 105.0, 95.0, 100.0))
            .collect();

        let context = build_timeframe_context("H4", &candles);
        assert!((context.atr14 - 10.0).abs() < 1e-9);
    }

    #[test]
    fn nearest_levels_fall_back_to_swing_high_low() {
        let mut candles: Vec<Candle> = (0..30)
            .map(|index| {
                let close = 100.0 + (index % 4) as f64;
                candle(
                    BASE_TS + index as i64 * STEP_MS,
                    close,
                    close + 2.0,
                    close - 2.0,
                    close,
                )
            })
            .collect();
        candles[29].close = 100.0;
        candles[29].high = 100.5;
        candles[29].low = 99.5;

        let context = build_timeframe_context("H1", &candles);
        let support = context.nearest_support.expect("fallback support");
        let resistance = context.nearest_resistance.expect("fallback resistance");
        assert!(support < context.close);
        assert!(resistance > context.close);
    }

    #[test]
    fn h4_filter_blocks_only_opposing_bias() {
        let bearish = context_with_h4_bias("BEARISH");
        let (allowed, reason) = h4_allows_signal("BUY", &bearish);
        assert!(!allowed);
        assert!(reason.contains("BEARISH"));
        assert!(h4_allows_signal("SELL", &bearish).0);

        let bullish = context_with_h4_bias("BULLISH");
        let (allowed, reason) = h4_allows_signal("SELL", &bullish);
        assert!(!allowed);
        assert!(reason.contains("BULLISH"));
        assert!(h4_allows_signal("BUY", &bullish).0);

        let neutral = context_with_h4_bias("NEUTRAL");
        assert!(h4_allows_signal("BUY", &neutral).0);
        assert!(h4_allows_signal("SELL", &neutral).0);
    }

    #[test]
    fn htf_filter_combines_h4_and_d1() {
        let mut context = context_with_h4_bias("NEUTRAL");
        context.d1.bias = "BEARISH".to_string();
        assert!(!htf_allows_signal("BUY", &context, false, true).0);
        assert!(htf_allows_signal("SELL", &context, false, true).0);
        assert!(htf_allows_signal("BUY", &context, false, false).0);

        let mut context = context_with_h4_bias("BULLISH");
        context.d1.bias = "BEARISH".to_string();
        assert!(!htf_allows_signal("BUY", &context, true, true).0);
        assert!(!htf_allows_signal("SELL", &context, true, true).0);
        assert!(htf_allows_signal("BUY", &context, true, false).0);
    }

    #[test]
    fn h4_filter_is_case_insensitive_and_allows_unknown_types() {
        let bearish = context_with_h4_bias("BEARISH");
        assert!(!h4_allows_signal(" buy ", &bearish).0);
        assert!(h4_allows_signal("UNKNOWN", &bearish).0);
    }

    #[test]
    fn build_market_context_populates_all_three_timeframes() {
        let d1 = trend_candles(220, 2000.0, 1.0);
        let h4 = trend_candles(220, 2000.0, 1.0);
        let h1 = trend_candles(220, 2000.0, 1.0);

        let context = build_market_context(&d1, &h4, &h1, "BINANCE_PAXG", Some(5.07));

        assert_eq!(context.d1.timeframe, "D1");
        assert_eq!(context.h4.timeframe, "H4");
        assert_eq!(context.h1.timeframe, "H1");
        assert_eq!(context.price_source, "BINANCE_PAXG");
        assert_eq!(context.spot_basis, Some(5.07));
        assert!(!context.updated_at.is_empty());
    }

    #[test]
    fn market_context_serializes_dashboard_contract() {
        let context = build_market_context(&[], &[], &[], "BINANCE_PAXG", None);
        let payload = serde_json::to_value(&context).expect("serialize context");

        for key in ["d1", "h4", "h1", "updated_at", "price_source", "spot_basis"] {
            assert!(payload.get(key).is_some(), "missing key: {}", key);
        }

        for key in [
            "timeframe",
            "bias",
            "close",
            "ema50",
            "ema200",
            "channel_position",
            "atr14",
            "nearest_support",
            "nearest_resistance",
            "candles",
        ] {
            assert!(
                payload["d1"].get(key).is_some(),
                "missing timeframe key: {}",
                key
            );
        }
        assert_eq!(payload["h4"]["bias"], "NEUTRAL");
        assert_eq!(payload["d1"]["candles"], 0);
    }

    #[test]
    fn shift_candles_prices_applies_basis_and_ignores_zero() {
        let mut candles = vec![candle(BASE_TS, 2000.1, 2005.55, 1995.0, 2002.34)];

        shift_candles_prices(&mut candles, 5.07);
        assert!((candles[0].open - 2005.17).abs() < 1e-9);
        assert!((candles[0].high - 2010.62).abs() < 1e-9);
        assert!((candles[0].low - 2000.07).abs() < 1e-9);
        assert!((candles[0].close - 2007.41).abs() < 1e-9);

        for basis in [0.0, f64::NAN, f64::INFINITY] {
            let mut untouched = vec![candle(BASE_TS, 2000.0, 2001.0, 1999.0, 2000.5)];
            shift_candles_prices(&mut untouched, basis);
            assert_eq!(untouched[0].open, 2000.0);
            assert_eq!(untouched[0].close, 2000.5);
        }
    }
}
