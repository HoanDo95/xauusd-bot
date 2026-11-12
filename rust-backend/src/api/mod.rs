use crate::divergence_alerts::DivergenceAlert;
use crate::market_context::{htf_allows_signal, MarketContext};
use crate::market_feed::MarketDataFeed;
use crate::models::{BotConfig, Candle, DailyMacroReport, Signal};
use crate::news_filter::NewsFilterService;
use crate::rsi::{divergence_label, rsi_allows_signal, rsi_snapshot, RsiSnapshot};
use crate::signal_review::{review_blocks_signal, review_signal};
use crate::storage::{PersistentState, PersistentStore};
use crate::telegram::TelegramNotifier;
use crate::tracker::evaluate_active_signals;
use crate::volume_delta::delta_allows_signal;
use axum::{
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::warn;

mod ai;
mod config;
mod divergence;
mod health;
mod live;
mod market;
mod news;
mod signals;
mod simulation;
mod telegram;
mod webhooks;

use ai::ai_review_signal;
use config::{calculate_lot, get_config, update_config};
use divergence::get_divergence_alerts;
use health::health_check;
use live::get_live_view;
use market::{get_htf_candles, get_market_context, get_recent_candles};
use news::{get_latest_macro_bias, get_news_calendar, trigger_daily_macro_cron};
use signals::{get_all_signals, get_stats};
use simulation::simulate_setup;
use telegram::{send_signal_telegram, trigger_test_telegram};
use webhooks::{receive_candle_webhook, receive_tradingview_webhook};

/// Trạng thái ứng dụng chia sẻ an toàn giữa các luồng (Thread-safe Shared State)
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<BotConfig>>,
    pub candles: Arc<RwLock<Vec<Candle>>>,
    pub signals: Arc<RwLock<Vec<Signal>>>,
    pub telegram: Arc<RwLock<TelegramNotifier>>,
    pub news_filter: Arc<RwLock<NewsFilterService>>,
    pub daily_report: Arc<RwLock<Option<DailyMacroReport>>>,
    /// Bộ lưu trữ bền vững `data/bot_state.json`.
    pub store: Arc<PersistentStore>,
    /// Nguồn dữ liệu nến thật Binance/TwelveData.
    pub market_feed: Arc<MarketDataFeed>,
    /// Giá live gần nhất từ nến đang chạy (nếu có).
    pub live_price: Arc<RwLock<Option<f64>>>,
    /// Nguồn dữ liệu thực tế đang hoạt động: TWELVEDATA / BINANCE_PAXG / FALLBACK_SEED.
    pub active_data_source: Arc<RwLock<String>>,
    /// Nguồn volume thực tế: BINANCE_PAXG / BINANCE_PAXG_PROXY / NONE.
    pub active_volume_source: Arc<RwLock<String>>,
    /// Giá spot XAU/USD tham chiếu gần nhất (TwelveData/gold-api.com, cache 300s).
    pub spot_price: Arc<RwLock<Option<f64>>>,
    /// Basis PAXG → spot đang áp dụng; None = chỉ dùng broker_price_offset thủ công.
    pub spot_basis: Arc<RwLock<Option<f64>>>,
    /// Dedup key các briefing trước phiên đã gửi (mới nhất ở cuối).
    pub sent_session_alerts: Arc<RwLock<Vec<String>>>,
    /// Bối cảnh D1/H4/H1 read-only (None khi chưa fetch được HTF).
    pub market_context: Arc<RwLock<Option<MarketContext>>>,
    /// Nến H4 đã đóng (đã căn spot basis) cho chart "view toàn cảnh".
    pub htf_h4_candles: Arc<RwLock<Vec<Candle>>>,
    /// Nến D1 đã đóng (đã căn spot basis) cho chart "view toàn cảnh".
    pub htf_d1_candles: Arc<RwLock<Vec<Candle>>>,
    /// Cổng tuần tự hóa bước cuối: dedup/quota -> Telegram -> lưu tín hiệu.
    /// Giữ khóa này xuyên suốt send/insert/persist để hai luồng không thể cùng
    /// vượt qua dedup/quota tại một thời điểm.
    pub signal_gate: Arc<Mutex<()>>,
    /// Tuần tự hóa việc ghi `data/bot_state.json` để các save đồng thời không
    /// giẫm lên cùng một file tạm.
    pub persist_lock: Arc<Mutex<()>>,
    /// Thời điểm tick quét gần nhất của scanner (dùng cho health trung thực).
    pub last_scan_at: Arc<RwLock<Option<chrono::DateTime<chrono::Utc>>>>,
    /// Thời điểm áp dụng spot basis hợp lệ gần nhất.
    pub spot_basis_at: Arc<RwLock<Option<chrono::DateTime<chrono::Utc>>>>,
    /// Cảnh báo phân kỳ RSI gần nhất (tối đa 50, chỉ giữ trong RAM).
    pub divergence_alerts: Arc<RwLock<Vec<DivergenceAlert>>>,
    /// Cooldown gửi cảnh báo theo khóa "{timeframe}:{direction}" (chỉ trong RAM).
    pub divergence_last_sent: Arc<RwLock<HashMap<String, DateTime<Utc>>>>,
}

/// Ghi snapshot Config + Signals + DailyReport + session alert dedup xuống đĩa
/// (không block async runtime).
pub async fn persist_state(state: &AppState) -> Result<(), String> {
    // Chỉ một save tại một thời điểm: tránh hai tác vụ cùng ghi file tạm
    // `bot_state.json.tmp` rồi rename đè lên nhau.
    let _persist_guard = state.persist_lock.lock().await;

    let snapshot = PersistentState {
        config: state.config.read().await.clone(),
        signals: state.signals.read().await.clone(),
        daily_report: state.daily_report.read().await.clone(),
        sent_session_alerts: state.sent_session_alerts.read().await.clone(),
    };
    let store = state.store.clone();

    tokio::task::spawn_blocking(move || store.save(&snapshot))
        .await
        .map_err(|err| format!("Không thể hoàn tất tác vụ ghi trạng thái: {}", err))?
}

/// Khởi tạo Axum Router với đầy đủ REST Endpoints, TradingView Webhook & Daily Scheduler
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/health", get(health_check))
        .route("/api/signals", get(get_all_signals))
        .route("/api/stats", get(get_stats))
        .route("/api/candles", get(get_recent_candles))
        .route("/api/candles/webhook", post(receive_candle_webhook))
        // Bối cảnh đa khung D1/H4/H1 (read-only, 204 khi chưa sẵn sàng)
        .route("/api/context", get(get_market_context))
        // Nến H4/D1 đã đóng cho chart "view toàn cảnh" (read-only)
        .route("/api/htf-candles", get(get_htf_candles))
        // Endpoint nhận TradingView Webhook Alert (cả 2 alias)
        .route("/api/webhook", post(receive_tradingview_webhook))
        .route(
            "/api/webhook/tradingview",
            post(receive_tradingview_webhook),
        )
        // Endpoint cấu hình, tính Lot & Telegram
        .route("/api/config", get(get_config).post(update_config))
        .route("/api/calculate-lot", post(calculate_lot))
        .route("/api/news-calendar", get(get_news_calendar))
        .route("/api/telegram/test", post(trigger_test_telegram))
        .route("/api/test-telegram", post(trigger_test_telegram))
        .route("/api/telegram/send-signal", post(send_signal_telegram))
        // Endpoint Daily Cron & Macro Connector
        .route("/api/macro-bias/latest", get(get_latest_macro_bias))
        .route(
            "/api/cron/trigger-daily-macro",
            post(trigger_daily_macro_cron),
        )
        // Endpoint giả lập setup (is_simulated = true, không tiêu hao quota thật)
        .route("/api/simulate-setup", post(simulate_setup))
        // Review thử tín hiệu bằng AI Risk Officer (không gửi Telegram, không tiêu hao quota)
        .route("/api/ai/review-signal", post(ai_review_signal))
        // Cảnh báo phân kỳ RSI độc lập (advisory, không phải entry, không tiêu hao quota)
        .route("/api/divergence-alerts", get(get_divergence_alerts))
        // Preview read-only: không gửi Telegram, không lưu tín hiệu, không tiêu hao quota
        .route("/api/live-view", get(get_live_view))
        .with_state(state)
}

/// Cập nhật TP1/TP2/SL cho các tín hiệu đang mở dựa trên nến hiện có.
pub(crate) async fn evaluate_outcomes(state: &AppState) -> usize {
    let candles = state.candles.read().await.clone();
    let updates = {
        let mut signals = state.signals.write().await;
        evaluate_active_signals(&mut signals, &candles)
    };
    if !updates.is_empty() {
        for update in updates.iter().filter(|signal| !signal.is_simulated) {
            let telegram = state.telegram.read().await.clone();
            if let Err(err) = telegram.send_outcome_alert(update).await {
                warn!("⚠️ Không gửi được Telegram outcome: {}", err);
            }
        }
        if let Err(err) = persist_state(state).await {
            warn!("⚠️ Không lưu được kết quả TP/SL: {}", err);
        }
    }
    updates.len()
}

/// Trả lý do chặn nếu bộ lọc H4/D1 đang bật, context sẵn sàng và tín hiệu bị chặn.
pub(crate) async fn htf_block_reason(
    state: &AppState,
    config: &BotConfig,
    signal_type: &str,
) -> Option<String> {
    if !config.htf_h4_filter_enabled && !config.htf_d1_filter_enabled {
        return None;
    }

    let guard = state.market_context.read().await;
    let context = guard.as_ref()?;
    let (allowed, reason) = htf_allows_signal(
        signal_type,
        context,
        config.htf_h4_filter_enabled,
        config.htf_d1_filter_enabled,
    );
    if allowed {
        None
    } else {
        Some(reason)
    }
}

/// Áp bộ lọc Volume Delta/CVD cho tín hiệu thật. Trả lý do nếu bị chặn;
/// nếu cho qua thì điền delta_ratio/cvd/delta vào signal.
pub(crate) fn apply_volume_delta_filter(
    config: &BotConfig,
    signal: &mut Signal,
    candles: &[Candle],
) -> Option<String> {
    if !config.volume_delta_filter_enabled || signal.is_simulated {
        return None;
    }

    let (allowed, reason, snapshot) =
        delta_allows_signal(signal.signal_type.as_str(), candles, config);
    if !allowed {
        return Some(reason);
    }

    signal.delta_ratio = Some(snapshot.delta_ratio);
    signal.cvd = Some(snapshot.cvd);
    signal.delta = Some(snapshot.delta);
    None
}

/// Gắn RSI context (M15) vào tín hiệu để dashboard/Telegram/AI cùng thấy.
pub(crate) fn attach_rsi_snapshot(signal: &mut Signal, snapshot: &RsiSnapshot) {
    signal.rsi_value = snapshot.value;
    signal.rsi_state = Some(snapshot.state.clone());
    signal.rsi_divergence = Some(divergence_label(snapshot.divergence));
}

/// Áp bộ lọc RSI cứng cho tín hiệu thật. Trả lý do nếu bị chặn.
/// Mặc định filter OFF => luôn None (không đổi hành vi).
pub(crate) fn rsi_block_reason(
    config: &BotConfig,
    signal: &Signal,
    snapshot: &RsiSnapshot,
) -> Option<String> {
    if !config.rsi_filter_enabled || signal.is_simulated {
        return None;
    }

    let (allowed, reason) = rsi_allows_signal(signal.signal_type.as_str(), snapshot, config);
    (!allowed).then_some(reason)
}

/// Chạy AI Risk Officer cho tín hiệu thật và trả lý do nếu bị chặn.
/// Tín hiệu giả lập bỏ qua hoàn toàn. AI không bao giờ sửa SL/TP.
pub(crate) async fn ai_review_block_reason(
    state: &AppState,
    config: &BotConfig,
    signal: &mut Signal,
) -> Option<String> {
    if !config.ai_signal_review_enabled || signal.is_simulated {
        return None;
    }

    let mut context = state.market_context.read().await.clone();
    if let Some(context) = context.as_mut() {
        let candles = state.candles.read().await.clone();
        context.rsi_m15 = Some(rsi_snapshot(
            &candles,
            config.rsi_period,
            config.rsi_overbought,
            config.rsi_oversold,
            config.rsi_divergence_lookback,
            config.rsi_pivot_k,
        ));
    }
    let events = state.news_filter.read().await.calendar_events.clone();
    match review_signal(signal, context.as_ref(), config, &events).await {
        Some(review) => {
            signal.ai_verdict = Some(review.action.clone());
            signal.ai_confidence = Some(review.confidence_score);
            signal.ai_reason = Some(review.reason.clone());
            let (blocked, reason) =
                review_blocks_signal(signal, &review, config.ai_signal_review_min_confidence);
            blocked.then_some(reason)
        }
        None => {
            if config.ai_signal_review_fail_open {
                warn!(
                    "⚠️ [AI Risk Officer] Không có review cho {} {} — fail-open, cho tín hiệu qua.",
                    signal.signal_type, signal.symbol
                );
                None
            } else {
                Some(format!(
                    "AI risk review unavailable cho {} {} — fail-closed (ai_signal_review_fail_open=false).",
                    signal.signal_type, signal.symbol
                ))
            }
        }
    }
}

/// Nhãn nguồn giá hiển thị cho dashboard: nêu rõ spot reference khi basis đang bật.
pub(crate) fn price_source_label(active_data_source: &str, spot_basis: Option<f64>) -> String {
    if spot_basis.is_some() && active_data_source.eq_ignore_ascii_case("BINANCE_PAXG") {
        "BINANCE_PAXG+TWELVEDATA_SPOT".to_string()
    } else {
        active_data_source.to_string()
    }
}

pub(crate) fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::config::{calculate_lot_response, default_current_price};
    use super::*;
    use crate::risk_manager::PositionSizer;

    #[test]
    fn default_price_prefers_live_then_last_close() {
        assert_eq!(default_current_price(Some(2690.5), Some(2680.0)), 2690.5);
        assert_eq!(default_current_price(None, Some(2680.0)), 2680.0);
        assert_eq!(default_current_price(Some(0.0), Some(2680.0)), 2680.0);
        assert_eq!(default_current_price(None, None), 2684.50);
    }

    #[test]
    fn calculate_lot_response_exposes_snake_and_camel_case_keys() {
        let result =
            PositionSizer::calculate_lot_size(10000.0, 1.0, 13.0, 26.0, 39.0, 2680.0, 100.0);
        let payload = calculate_lot_response(&result);

        for key in [
            "lot_size",
            "risk_amount_usd",
            "potential_profit_tp1_usd",
            "potential_profit_tp2_usd",
            "estimated_margin_usd",
            "risk_assessment",
            "lotSize",
            "riskAmountUsd",
            "potentialProfitTp1Usd",
            "potentialProfitTp2Usd",
            "estimatedMarginUsd",
            "riskAssessment",
        ] {
            assert!(payload.get(key).is_some(), "missing key: {}", key);
        }

        assert_eq!(payload["lot_size"], 0.07);
        assert_eq!(payload["lotSize"], 0.07);
        assert_eq!(payload["risk_amount_usd"], 91.0);
        assert_eq!(payload["riskAmountUsd"], 91.0);
        assert_eq!(payload["potential_profit_tp1_usd"], 182.0);
        assert_eq!(payload["potentialProfitTp1Usd"], 182.0);
        assert_eq!(payload["potential_profit_tp2_usd"], 273.0);
        assert_eq!(payload["potentialProfitTp2Usd"], 273.0);
    }

    #[test]
    fn price_source_labels_spot_calibrated_binance_feed() {
        assert_eq!(
            price_source_label("BINANCE_PAXG", Some(5.07)),
            "BINANCE_PAXG+TWELVEDATA_SPOT"
        );
        assert_eq!(price_source_label("BINANCE_PAXG", None), "BINANCE_PAXG");
        assert_eq!(
            price_source_label("FALLBACK_SEED", Some(5.07)),
            "FALLBACK_SEED"
        );
    }
}
