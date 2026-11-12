use super::{price_source_label, AppState};
use crate::analyzer::GoldMarketAnalyzer;
use crate::rsi::rsi_snapshot;
use crate::volume_delta::compute_delta;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

/// Lấy nến hiện tại phục vụ vẽ biểu đồ + S/R động + EMA trend.
pub(super) async fn get_recent_candles(State(state): State<AppState>) -> impl IntoResponse {
    let candles = state.candles.read().await.clone();
    let config = state.config.read().await.clone();

    let (supports, resistances) =
        GoldMarketAnalyzer::detect_dynamic_sr_zones(&candles, config.lookback_period.max(5), 2.5);
    let volume_ma =
        GoldMarketAnalyzer::calculate_volume_ma(&candles, config.volume_ma_period.max(1));
    let last_closed_price = candles.last().map(|candle| candle.close).unwrap_or(0.0);
    let live_price = *state.live_price.read().await;
    let current_price = live_price.unwrap_or(last_closed_price);
    let active_data_source = state.active_data_source.read().await.clone();
    let active_volume_source = state.active_volume_source.read().await.clone();
    let spot_price = *state.spot_price.read().await;
    let spot_basis = *state.spot_basis.read().await;
    let price_source = price_source_label(&active_data_source, spot_basis);
    let ema50 = GoldMarketAnalyzer::calculate_ema(&candles, 50);
    let ema200 = GoldMarketAnalyzer::calculate_ema(&candles, 200);
    let trend_bias = GoldMarketAnalyzer::detect_ema_trend_bias(&candles);

    Json(json!({
        "count": candles.len(),
        "candles": candles,
        "supports": supports,
        "resistances": resistances,
        "volumeMA": (volume_ma * 100.0).round() / 100.0,
        "currentPrice": current_price,
        "lastClosedPrice": last_closed_price,
        "dataSource": active_data_source,
        "volumeSource": active_volume_source,
        "priceSource": price_source,
        "spotPrice": spot_price,
        "spotBasis": spot_basis,
        "ema50": (ema50 * 100.0).round() / 100.0,
        "ema200": (ema200 * 100.0).round() / 100.0,
        "trendBias": trend_bias.as_str(),
        "volumeDelta": compute_delta(&candles, config.volume_delta_lookback)
    }))
}

/// Bối cảnh D1/H4/H1 read-only cho dashboard. 204 khi chưa fetch được HTF.
/// RSI M15 được tính live từ `state.candles` theo config hiện tại.
pub(super) async fn get_market_context(State(state): State<AppState>) -> impl IntoResponse {
    let context = state.market_context.read().await.clone();
    match context {
        Some(mut context) => {
            let config = state.config.read().await.clone();
            let candles = state.candles.read().await.clone();
            context.rsi_m15 = Some(rsi_snapshot(
                &candles,
                config.rsi_period,
                config.rsi_overbought,
                config.rsi_oversold,
                config.rsi_divergence_lookback,
                config.rsi_pivot_k,
            ));
            (StatusCode::OK, Json(context)).into_response()
        }
        None => StatusCode::NO_CONTENT.into_response(),
    }
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct HtfCandlesQuery {
    #[serde(default)]
    timeframe: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// GET /api/htf-candles?timeframe=H4&limit=120 — nến H4/D1 đã đóng cho chart toàn cảnh.
pub(super) async fn get_htf_candles(
    State(state): State<AppState>,
    Query(query): Query<HtfCandlesQuery>,
) -> impl IntoResponse {
    let timeframe = query
        .timeframe
        .as_deref()
        .unwrap_or("H4")
        .trim()
        .to_uppercase();
    let (label, candles) = match timeframe.as_str() {
        "D1" => ("D1", state.htf_d1_candles.read().await.clone()),
        _ => ("H4", state.htf_h4_candles.read().await.clone()),
    };
    let limit = query.limit.unwrap_or(120).clamp(20, 300);
    let start = candles.len().saturating_sub(limit);

    Json(json!({
        "timeframe": label,
        "candles": &candles[start..]
    }))
}
