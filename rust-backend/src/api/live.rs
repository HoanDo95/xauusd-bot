use super::{round2, AppState};
use crate::analyzer::GoldMarketAnalyzer;
use crate::market_context::{htf_allows_signal, MarketContext};
use crate::models::{BotConfig, Candle};
use crate::rsi::{divergence_label, rsi_allows_signal, rsi_snapshot};
use crate::volume_delta::{compute_delta, delta_allows_signal};
use axum::{extract::State, response::IntoResponse, Json};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

const NO_DATA_MESSAGE: &str = "Chưa đủ nến để phân tích.";
const NO_SETUP_MESSAGE: &str = "Chưa đủ điều kiện Pinbar + Volume + vùng S/R.";

/// GET /api/live-view — read-only preview of the setup the scanner sees right now.
///
/// This handler never mutates state, sends Telegram, inserts/persists a signal, or
/// consumes the daily Sniper quota. AI review is only reported as a config flag;
/// it is never called here (latency/cost). Safe to call while the scanner is paused.
pub(super) async fn get_live_view(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let candles = state.candles.read().await.clone();
    let live_price = *state.live_price.read().await;
    let market_context = state.market_context.read().await.clone();
    let active_data_source = state.active_data_source.read().await.clone();
    let active_volume_source = state.active_volume_source.read().await.clone();
    let now_ms = Utc::now().timestamp_millis();

    Json(build_live_view(
        &candles,
        &config,
        live_price,
        market_context.as_ref(),
        &active_data_source,
        &active_volume_source,
        now_ms,
    ))
}

/// Pure response builder for `/api/live-view` (no locks, no side effects).
///
/// Filters are evaluated only for display: a `setup` response may still show
/// `*_ok=false`, meaning the real scanner would skip that signal.
pub(super) fn build_live_view(
    candles: &[Candle],
    config: &BotConfig,
    live_price: Option<f64>,
    context: Option<&MarketContext>,
    active_data_source: &str,
    active_volume_source: &str,
    now_ms: i64,
) -> Value {
    if candles.len() < config.lookback_period + 5 {
        return json!({
            "status": "no_data",
            "message": NO_DATA_MESSAGE,
            "candles": candles.len(),
        });
    }

    let generated_at = DateTime::<Utc>::from_timestamp_millis(now_ms)
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339());

    let Some(signal) =
        GoldMarketAnalyzer::analyze_market_with_live_price(candles, config, live_price, now_ms)
    else {
        let (candle_timestamp, last_close) = candles
            .last()
            .map(|candle| (candle.timestamp, round2(candle.close)))
            .unwrap_or((0, 0.0));

        return json!({
            "status": "no_setup",
            "message": NO_SETUP_MESSAGE,
            "candle_timestamp": candle_timestamp,
            "last_close": last_close,
            "source": active_data_source,
            "volumeSource": active_volume_source,
            "generated_at": generated_at,
        });
    };

    let (htf_ok, htf_reason) = match context {
        Some(context) => htf_allows_signal(
            signal.signal_type.as_str(),
            context,
            config.htf_h4_filter_enabled,
            config.htf_d1_filter_enabled,
        ),
        None => (
            true,
            "Chưa có bối cảnh D1/H4 — bỏ qua bộ lọc HTF.".to_string(),
        ),
    };

    let rsi = rsi_snapshot(
        candles,
        config.rsi_period,
        config.rsi_overbought,
        config.rsi_oversold,
        config.rsi_divergence_lookback,
        config.rsi_pivot_k,
    );
    let (rsi_ok, rsi_reason) = if config.rsi_filter_enabled {
        rsi_allows_signal(signal.signal_type.as_str(), &rsi, config)
    } else {
        (true, "RSI filter disabled".to_string())
    };
    let rsi_divergence = divergence_label(rsi.divergence);
    let rsi_value = rsi.value;
    let rsi_state = rsi.state;

    let delta = compute_delta(candles, config.volume_delta_lookback);
    let (delta_ok, delta_reason) = if config.volume_delta_filter_enabled {
        let (allowed, reason, _) =
            delta_allows_signal(signal.signal_type.as_str(), candles, config);
        (allowed, reason)
    } else {
        (true, "Volume Delta filter disabled".to_string())
    };

    json!({
        "status": "setup",
        "signal": signal,
        "filters": {
            "htf_ok": htf_ok,
            "htf_reason": htf_reason,
            "rsi_ok": rsi_ok,
            "rsi_reason": rsi_reason,
            "rsi_value": rsi_value,
            "rsi_state": rsi_state,
            "rsi_divergence": rsi_divergence,
            "delta_ok": delta_ok,
            "delta_reason": delta_reason,
            "delta_ratio": delta.delta_ratio,
            "cvd": delta.cvd,
            "ai_review_enabled": config.ai_signal_review_enabled,
        },
        "source": active_data_source,
        "volumeSource": active_volume_source,
        "generated_at": generated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;

    const BASE_TS: i64 = 1_700_000_000_000;
    const STEP_MS: i64 = 15 * 60 * 1000;

    fn candle(timestamp: i64, open: f64, high: f64, low: f64, close: f64, volume: f64) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    /// Zigzag series with stable swing highs/lows; the last candle is too small
    /// to be a valid Pinbar, so the analyzer returns no setup.
    fn zigzag_candles(count: usize) -> Vec<Candle> {
        (0..count)
            .map(|index| {
                let level = match index % 4 {
                    0 => 2670.0,
                    2 => 2690.0,
                    _ => 2680.0,
                };
                candle(
                    BASE_TS + index as i64 * STEP_MS,
                    level,
                    level + 0.5,
                    level - 0.5,
                    level,
                    1000.0,
                )
            })
            .collect()
    }

    fn bullish_pinbar(timestamp: i64) -> Candle {
        candle(timestamp, 2674.0, 2675.6, 2669.5, 2675.0, 2000.0)
    }

    fn buy_config() -> BotConfig {
        BotConfig {
            gold_sl_buffer_points: 10.0,
            lookback_period: 30,
            volume_ma_period: 20,
            min_risk_reward: 2.0,
            volume_surge_multiplier: 1.5,
            rejection_wick_ratio: 2.0,
            macro_bias: "BULLISH".to_string(),
            account_balance: 10_000.0,
            risk_percentage: 1.0,
            ..BotConfig::default()
        }
    }

    fn build(candles: &[Candle]) -> Value {
        build_live_view(
            candles,
            &buy_config(),
            None,
            None,
            "TWELVEDATA",
            "BINANCE_PAXG_PROXY",
            BASE_TS,
        )
    }

    #[test]
    fn no_data_when_candles_are_below_lookback_requirement() {
        let candles = zigzag_candles(10);
        let view = build(&candles);

        assert_eq!(view["status"], "no_data");
        assert_eq!(view["message"], NO_DATA_MESSAGE);
        assert_eq!(view["candles"], 10);
    }

    #[test]
    fn no_setup_reports_last_candle_and_sources() {
        let candles = zigzag_candles(60);
        let view = build(&candles);

        assert_eq!(view["status"], "no_setup");
        assert_eq!(view["message"], NO_SETUP_MESSAGE);
        assert_eq!(view["candle_timestamp"], candles[59].timestamp);
        assert_eq!(view["last_close"], 2680.0);
        assert_eq!(view["source"], "TWELVEDATA");
        assert_eq!(view["volumeSource"], "BINANCE_PAXG_PROXY");
        assert!(view.get("signal").is_none());
        assert!(view["generated_at"].as_str().is_some());
    }

    #[test]
    fn setup_returns_signal_and_read_only_filter_context() {
        let mut candles = zigzag_candles(60);
        candles.push(bullish_pinbar(BASE_TS + 60 * STEP_MS));
        let view = build(&candles);

        assert_eq!(view["status"], "setup");
        assert_eq!(view["signal"]["signal_type"], "BUY");
        assert_eq!(view["signal"]["symbol"], "XAU/USD");
        assert_eq!(view["source"], "TWELVEDATA");
        assert_eq!(view["volumeSource"], "BINANCE_PAXG_PROXY");

        let filters = &view["filters"];
        assert_eq!(filters["htf_ok"], true);
        assert!(filters["htf_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("Chưa có bối cảnh"));
        assert_eq!(filters["rsi_ok"], true);
        assert!(filters["rsi_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("disabled"));
        assert!(filters["rsi_value"].is_number());
        assert_eq!(filters["rsi_state"], "Neutral");
        assert_eq!(filters["rsi_divergence"], "None");
        assert_eq!(filters["delta_ok"], true);
        assert!(filters["delta_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("disabled"));
        assert!(filters["delta_ratio"].is_number());
        assert!(filters["cvd"].is_number());
        assert_eq!(filters["ai_review_enabled"], false);
    }

    #[test]
    fn htf_filter_reports_block_without_dropping_preview() {
        let mut candles = zigzag_candles(60);
        candles.push(bullish_pinbar(BASE_TS + 60 * STEP_MS));

        let mut context =
            crate::market_context::build_market_context(&[], &[], &[], "TWELVEDATA", None);
        context.h4.bias = "BEARISH".to_string();
        let config = BotConfig {
            htf_h4_filter_enabled: true,
            ..buy_config()
        };

        let view = build_live_view(
            &candles,
            &config,
            None,
            Some(&context),
            "TWELVEDATA",
            "BINANCE_PAXG_PROXY",
            BASE_TS,
        );

        assert_eq!(view["status"], "setup");
        assert_eq!(view["filters"]["htf_ok"], false);
        assert!(view["filters"]["htf_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("BEARISH"));
    }
}
