use super::{attach_rsi_snapshot, persist_state, round2, AppState};
use crate::analyzer::GoldMarketAnalyzer;
use crate::models::{BotConfig, Candle};
use crate::rsi::rsi_snapshot;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

#[derive(Debug, Default, Deserialize)]
pub(super) struct SimulateSetupRequest {
    #[serde(rename = "type", default)]
    setup_type: String,
}

/// Giả lập nhanh 1 setup (BUY_PINBAR / SELL_PINBAR / RANDOM_CANDLE).
/// Tín hiệu tạo ra được đánh dấu `is_simulated = true` nên KHÔNG tiêu hao quota 1 lệnh thật/ngày.
pub(super) async fn simulate_setup(
    State(state): State<AppState>,
    Json(payload): Json<SimulateSetupRequest>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let mut candles_snapshot = state.candles.read().await.clone();
    let now_ms = Utc::now().timestamp_millis();

    let last_close = candles_snapshot
        .last()
        .map(|candle| candle.close)
        .unwrap_or(2684.50);
    let volume_ma =
        GoldMarketAnalyzer::calculate_volume_ma(&candles_snapshot, config.volume_ma_period.max(1))
            .max(1.0);
    let (supports, resistances) = GoldMarketAnalyzer::detect_dynamic_sr_zones(
        &candles_snapshot,
        config.lookback_period.max(5),
        2.5,
    );

    let setup_type = payload.setup_type.to_uppercase();
    let new_candle = match setup_type.as_str() {
        "BUY_PINBAR" => {
            // Chọn hỗ trợ gần giá live nhất trong ±20$ để kịch bản test thực tế,
            // tránh dùng một vùng cản quá xa khiến entry trông sai lệch.
            let support_price = supports
                .iter()
                .filter(|zone| zone.price < last_close && last_close - zone.price <= 20.0)
                .max_by(|a, b| a.price.total_cmp(&b.price))
                .map(|zone| zone.price)
                .unwrap_or(last_close - 2.0);
            let open = support_price + 2.5;
            let low = support_price - 1.2;
            let close = support_price + 3.8;
            let high = close + 0.6;
            Candle {
                timestamp: now_ms,
                open: round2(open),
                high: round2(high),
                low: round2(low),
                close: round2(close),
                volume: round2(volume_ma * (config.volume_surge_multiplier + 0.35)),
                taker_buy_volume: 0.0,
                timeframe: config.timeframe,
            }
        }
        "SELL_PINBAR" => {
            let resistance_price = resistances
                .iter()
                .filter(|zone| zone.price > last_close && zone.price - last_close <= 20.0)
                .min_by(|a, b| a.price.total_cmp(&b.price))
                .map(|zone| zone.price)
                .unwrap_or(last_close + 2.0);
            let open = resistance_price - 2.5;
            let high = resistance_price + 1.4;
            let close = resistance_price - 3.5;
            let low = close - 0.5;
            Candle {
                timestamp: now_ms,
                open: round2(open),
                high: round2(high),
                low: round2(low),
                close: round2(close),
                volume: round2(volume_ma * (config.volume_surge_multiplier + 0.4)),
                taker_buy_volume: 0.0,
                timeframe: config.timeframe,
            }
        }
        _ => {
            // RANDOM_CANDLE: biến động nhẹ quanh giá hiện tại (deterministic từ thời gian).
            let phase = (now_ms % 1000) as f64 / 1000.0;
            let delta = (phase - 0.49) * 4.0;
            let open = last_close;
            let close = round2(open + delta);
            let high = round2(open.max(close) + 1.5);
            let low = round2(open.min(close) - 1.5);
            Candle {
                timestamp: now_ms,
                open,
                high,
                low,
                close,
                volume: round2(volume_ma * (0.9 + phase * 0.5)),
                taker_buy_volume: 0.0,
                timeframe: config.timeframe,
            }
        }
    };

    candles_snapshot.push(new_candle.clone());
    if candles_snapshot.len() > 500 {
        candles_snapshot.remove(0);
    }

    // Simulation dùng snapshot cục bộ: KHÔNG ghi nến giả vào state.candles và
    // KHÔNG đánh giá outcome của các tín hiệu thật bằng dữ liệu giả.
    let sim_config = BotConfig {
        macro_bias: "NEUTRAL".to_string(),
        ..config
    };
    // Dùng giá live thật (nếu có) để kiểm tra đúng logic drift/entry mode.
    // Nếu chưa có live price, fallback về close của nến giả lập.
    let actual_live_price = *state.live_price.read().await;
    let detected_signal = GoldMarketAnalyzer::analyze_market_with_live_price(
        &candles_snapshot,
        &sim_config,
        actual_live_price.or(Some(new_candle.close)),
        now_ms,
    );

    if let Some(mut signal) = detected_signal {
        signal.is_simulated = true;
        let rsi = rsi_snapshot(
            &candles_snapshot,
            config.rsi_period,
            config.rsi_overbought,
            config.rsi_oversold,
            config.rsi_divergence_lookback,
            config.rsi_pivot_k,
        );
        attach_rsi_snapshot(&mut signal, &rsi);
        signal.price_action_notes = format!("[SIMULATION] {}", signal.price_action_notes);
        signal.full_rationale =
            format!("[SIMULATION - KHÔNG TÍNH QUOTA] {}", signal.full_rationale);

        let telegram = state.telegram.read().await.clone();
        if let Ok(msg_id) = telegram.send_signal_alert(&signal).await {
            signal.telegram_message_id = Some(msg_id);
        }

        state.signals.write().await.insert(0, signal.clone());
        if let Err(err) = persist_state(&state).await {
            warn!("⚠️ Không lưu được simulated signal: {}", err);
        }

        return (
            StatusCode::OK,
            Json(json!({
                "status": "signal_triggered",
                "is_simulated": true,
                "candle": new_candle,
                "signal": signal
            })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "candle_added",
            "is_simulated": true,
            "candle": new_candle,
            "message": "Đã phân tích setup trên snapshot tạm. Chưa hội tụ đủ điều kiện Pinbar + Volume tại vùng cản."
        })),
    )
}
