use crate::analyzer::GoldMarketAnalyzer;
use crate::market_feed::MarketDataFeed;
use crate::models::{
    BotConfig, Candle, DailyMacroReport, NewCandleWebhook, Signal, SignalStatus, SignalType,
    Timeframe, TradingViewWebhookPayload,
};
use crate::news_filter::{MacroBias, NewsFilterService};
use crate::risk_manager::PositionSizer;
use crate::scheduler::DailyScheduler;
use crate::storage::{PersistentState, PersistentStore};
use crate::telegram::TelegramNotifier;
use crate::tracker::{compute_stats, evaluate_active_signals};
use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

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
}

/// Ghi snapshot Config + Signals + DailyReport xuống đĩa (không block async runtime).
pub async fn persist_state(state: &AppState) -> Result<(), String> {
    let snapshot = PersistentState {
        config: state.config.read().await.clone(),
        signals: state.signals.read().await.clone(),
        daily_report: state.daily_report.read().await.clone(),
    };
    let store = state.store.clone();

    tokio::task::spawn_blocking(move || store.save(&snapshot))
        .await
        .map_err(|err| format!("Không thể hoàn tất tác vụ ghi trạng thái: {}", err))?
}

/// Cập nhật TP1/TP2/SL cho các tín hiệu đang mở dựa trên nến hiện có.
async fn evaluate_outcomes(state: &AppState) -> usize {
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

/// Khởi tạo Axum Router với đầy đủ REST Endpoints, TradingView Webhook & Daily Scheduler
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/health", get(health_check))
        .route("/api/signals", get(get_all_signals))
        .route("/api/stats", get(get_stats))
        .route("/api/candles", get(get_recent_candles))
        .route("/api/candles/webhook", post(receive_candle_webhook))
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
        // Endpoint Daily Cron & Macro Connector
        .route("/api/macro-bias/latest", get(get_latest_macro_bias))
        .route(
            "/api/cron/trigger-daily-macro",
            post(trigger_daily_macro_cron),
        )
        // Endpoint giả lập setup (is_simulated = true, không tiêu hao quota thật)
        .route("/api/simulate-setup", post(simulate_setup))
        .with_state(state)
}

async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let signals = state.signals.read().await.clone();
    let candles_count = state.candles.read().await.len();
    let now = Utc::now();

    let signals_today = PersistentStore::count_live_signals_today(&signals, now);
    let daily_quota_reached =
        PersistentStore::has_reached_daily_quota(&signals, config.max_signals_per_day, now);
    let is_market_safe = state
        .news_filter
        .read()
        .await
        .check_kill_switch(now, &config);

    Json(json!({
        "status": "healthy",
        "service": "Gold Signal Bot (Rust Backend)",
        "engine": "Tokio 1.40 / Axum 0.7 High Performance",
        "asset": "XAU/USD",
        "version": "1.3.0",
        "symbol": config.symbol,
        "timeframe": config.timeframe,
        "isActive": config.is_active,
        "candlesCount": candles_count,
        "signalsCount": signals.len(),
        "signalsToday": signals_today,
        "maxSignalsPerDay": config.max_signals_per_day,
        "dailyQuotaReached": daily_quota_reached,
        "sessionFilterEnabled": config.session_filter_enabled,
        "dataSource": config.data_source,
        "killSwitchActive": !is_market_safe,
        "cron_schedule": "07:00 AM Daily",
        "tradingview_webhook": "/api/webhook/tradingview"
    }))
}

/// Lấy danh sách các tín hiệu đã sinh ra + trạng thái quota Sniper trong ngày.
async fn get_all_signals(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let signals = state.signals.read().await.clone();
    let now = Utc::now();
    let signals_today = PersistentStore::count_live_signals_today(&signals, now);
    let daily_quota_reached =
        PersistentStore::has_reached_daily_quota(&signals, config.max_signals_per_day, now);
    let stats = compute_stats(&signals);

    Json(json!({
        "total": signals.len(),
        "signals": signals,
        "stats": stats,
        "signalsToday": signals_today,
        "maxSignalsPerDay": config.max_signals_per_day,
        "dailyQuotaReached": daily_quota_reached
    }))
}

/// Hiệu suất tích lũy của các tín hiệu thật (TP1/TP2/SL).
async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    let signals = state.signals.read().await.clone();
    Json(compute_stats(&signals))
}

/// Lấy nến hiện tại phục vụ vẽ biểu đồ + S/R động + EMA trend.
async fn get_recent_candles(State(state): State<AppState>) -> impl IntoResponse {
    let candles = state.candles.read().await.clone();
    let config = state.config.read().await.clone();

    let (supports, resistances) =
        GoldMarketAnalyzer::detect_dynamic_sr_zones(&candles, config.lookback_period.max(5), 2.5);
    let volume_ma =
        GoldMarketAnalyzer::calculate_volume_ma(&candles, config.volume_ma_period.max(1));
    let current_price = candles.last().map(|candle| candle.close).unwrap_or(0.0);
    let ema50 = GoldMarketAnalyzer::calculate_ema(&candles, 50);
    let trend_bias = GoldMarketAnalyzer::detect_ema_trend_bias(&candles);

    Json(json!({
        "count": candles.len(),
        "candles": candles,
        "supports": supports,
        "resistances": resistances,
        "volumeMA": (volume_ma * 100.0).round() / 100.0,
        "currentPrice": current_price,
        "ema50": (ema50 * 100.0).round() / 100.0,
        "trendBias": trend_bias.as_str()
    }))
}

/// Webhook nhận nến mới từ sàn giao dịch (Binance / MetaTrader EA).
/// Enforce: Daily Quota -> Dedup -> News Kill Switch -> Analyzer.
async fn receive_candle_webhook(
    State(state): State<AppState>,
    Json(payload): Json<NewCandleWebhook>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();

    if !config.webhook_passphrase.is_empty()
        && payload.passphrase.as_deref() != Some(config.webhook_passphrase.as_str())
    {
        warn!("🔒 [Candle Webhook] Từ chối passphrase không hợp lệ.");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "unauthorized",
                "message": "Webhook passphrase không hợp lệ."
            })),
        );
    }

    let timeframe = match payload.timeframe.to_uppercase().as_str() {
        "H1" => Timeframe::H1,
        "H4" => Timeframe::H4,
        _ => Timeframe::M15,
    };

    let new_candle = Candle {
        timestamp: payload
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis()),
        open: payload.open,
        high: payload.high,
        low: payload.low,
        close: payload.close,
        volume: payload.volume,
        timeframe,
    };

    push_candle(&state, new_candle.clone()).await;
    evaluate_outcomes(&state).await;

    if !config.is_active {
        return (
            StatusCode::OK,
            Json(json!({
                "status": "bot_inactive",
                "message": "Nến đã được ghi nhận nhưng bot đang ở trạng thái TẠM DỪNG"
            })),
        );
    }

    let now = Utc::now();
    let signals_snapshot = state.signals.read().await.clone();

    // 1. Daily Sniper Quota (mặc định 1 lệnh thật/ngày, giờ Việt Nam UTC+7)
    if PersistentStore::has_reached_daily_quota(&signals_snapshot, config.max_signals_per_day, now)
    {
        return (
            StatusCode::OK,
            Json(json!({
                "status": "daily_quota_reached",
                "message": format!(
                    "🎯 Đã đạt quota {} lệnh thật hôm nay. Bot nghỉ ngơi tới ngày giao dịch kế tiếp.",
                    config.max_signals_per_day
                ),
                "signalsToday": PersistentStore::count_live_signals_today(&signals_snapshot, now),
                "maxSignalsPerDay": config.max_signals_per_day
            })),
        );
    }

    // 2. Dedup cùng một cây nến
    let dedup_key = format!(
        "{}_{}_{}",
        config.symbol, config.timeframe, new_candle.timestamp
    );
    if PersistentStore::is_duplicate_candle(&signals_snapshot, &dedup_key) {
        return (
            StatusCode::OK,
            Json(json!({
                "status": "duplicate_candle",
                "message": "Cây nến này đã được quét trước đó (dedup_key).",
                "dedup_key": dedup_key
            })),
        );
    }

    // 3. News Kill Switch
    let is_news_safe = state
        .news_filter
        .read()
        .await
        .check_kill_switch(now, &config);
    if !is_news_safe {
        warn!("🛑 [Candle Webhook] Bỏ qua do News Kill Switch đang khóa.");
        return (
            StatusCode::OK,
            Json(json!({
                "status": "kill_switch_active",
                "message": "Kill Switch đang bật: đang trong cửa sổ ±30 phút quanh tin High Impact USD."
            })),
        );
    }

    // 4. Chạy bộ máy phân tích thuật toán (chỉ trên cây nến vừa gửi)
    let candles_snapshot = state.candles.read().await.clone();
    if let Some(mut signal) = GoldMarketAnalyzer::analyze_market(&candles_snapshot, &config) {
        if PersistentStore::is_duplicate_candle(&signals_snapshot, &signal.dedup_key) {
            return (
                StatusCode::OK,
                Json(json!({
                    "status": "duplicate_candle",
                    "message": "Tín hiệu cho cây nến này đã tồn tại.",
                    "dedup_key": signal.dedup_key
                })),
            );
        }

        info!(
            "🎯 [Candle Webhook] Thuật toán phát hiện tín hiệu: {}",
            signal.id
        );
        signal.is_simulated = false;

        let telegram = state.telegram.read().await.clone();
        if let Ok(msg_id) = telegram.send_signal_alert(&signal).await {
            signal.telegram_message_id = Some(msg_id);
        }

        state.signals.write().await.insert(0, signal.clone());
        if let Err(err) = persist_state(&state).await {
            warn!("⚠️ Không lưu được trạng thái sau tín hiệu: {}", err);
        }

        return (
            StatusCode::CREATED,
            Json(json!({
                "status": "signal_triggered",
                "signal": signal
            })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "no_signal",
            "message": "Điều kiện Pinbar hoặc Khối lượng hấp thụ chưa thỏa mãn tại vùng cản"
        })),
    )
}

/// Webhook tiếp nhận cảnh báo tín hiệu trực tiếp từ TradingView.
/// Tích hợp đầy đủ Daily Quota, Dedup, Kill Switch, Macro Bias Filter và Lot Size.
async fn receive_tradingview_webhook(
    State(state): State<AppState>,
    Json(payload): Json<TradingViewWebhookPayload>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let now = Utc::now();

    if !config.webhook_passphrase.is_empty()
        && payload.passphrase.as_deref() != Some(config.webhook_passphrase.as_str())
    {
        warn!("🔒 [TradingView Webhook] Từ chối passphrase không hợp lệ.");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "unauthorized",
                "action_taken": "DROPPED",
                "reason": "Webhook passphrase không hợp lệ."
            })),
        );
    }

    info!(
        "📥 [TradingView Webhook] Nhận tín hiệu từ {}: action={}, price={:.2}",
        payload.ticker, payload.action, payload.price
    );

    if !config.is_active {
        return (
            StatusCode::OK,
            Json(json!({
                "status": "bot_inactive",
                "action_taken": "DROPPED",
                "reason": "Bot đang ở trạng thái TẠM DỪNG."
            })),
        );
    }

    let action_upper = payload.action.to_uppercase();

    // Nếu payload là cây nến (action == "CANDLE") thì chỉ ghi dữ liệu.
    if action_upper == "CANDLE" {
        let candle = Candle {
            timestamp: payload.timestamp.unwrap_or_else(|| now.timestamp_millis()),
            open: payload.open.unwrap_or(payload.price),
            high: payload.high.unwrap_or(payload.price + 1.0),
            low: payload.low.unwrap_or(payload.price - 1.0),
            close: payload.close.unwrap_or(payload.price),
            volume: payload.volume.unwrap_or(1500.0),
            timeframe: Timeframe::M15,
        };
        push_candle(&state, candle.clone()).await;
        evaluate_outcomes(&state).await;

        return (
            StatusCode::OK,
            Json(json!({
                "status": "candle_recorded",
                "message": "Nến TradingView đã lưu vào buffer phân tích.",
                "candle": candle
            })),
        );
    }

    let is_buy = match action_upper.as_str() {
        "BUY" => true,
        "SELL" => false,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "invalid_action",
                    "message": "action phải là BUY, SELL hoặc CANDLE"
                })),
            )
        }
    };

    // Bước 1: News Kill Switch
    let (is_news_safe, active_event) = {
        let nf = state.news_filter.read().await;
        (
            nf.check_kill_switch(now, &config),
            nf.get_current_blocking_event(now, &config).cloned(),
        )
    };
    if !is_news_safe {
        let event_title = active_event
            .map(|event| event.title)
            .unwrap_or_else(|| "Tin tức Đỏ High Impact".to_string());
        warn!(
            "🛑 [TradingView Webhook BỊ CHẶN] Kill Switch đang khóa do tin tức: {}",
            event_title
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "blocked_by_kill_switch",
                "action_taken": "DROPPED",
                "reason": format!("🛑 Tự động hủy lệnh: Sắp ra hoặc vừa ra tin lớn [{}] trong phạm vi 30 phút!", event_title),
                "event": event_title,
                "current_time": now.to_rfc3339()
            })),
        );
    }

    // Bước 2: Daily Sniper Quota
    let signals_snapshot = state.signals.read().await.clone();
    if PersistentStore::has_reached_daily_quota(&signals_snapshot, config.max_signals_per_day, now)
    {
        return (
            StatusCode::OK,
            Json(json!({
                "status": "blocked_daily_quota",
                "action_taken": "DROPPED",
                "reason": format!(
                    "🎯 Đã đạt quota {} lệnh thật hôm nay (giờ Việt Nam UTC+7).",
                    config.max_signals_per_day
                ),
                "signalsToday": PersistentStore::count_live_signals_today(&signals_snapshot, now),
                "maxSignalsPerDay": config.max_signals_per_day
            })),
        );
    }

    // Bước 3: Dedup cùng cây nến / cùng timestamp webhook
    let candle_timestamp = payload.timestamp.unwrap_or_else(|| now.timestamp_millis());
    let dedup_key = format!(
        "{}_{}_{}",
        payload.ticker, config.timeframe, candle_timestamp
    );
    if PersistentStore::is_duplicate_candle(&signals_snapshot, &dedup_key) {
        return (
            StatusCode::OK,
            Json(json!({
                "status": "duplicate_candle",
                "action_taken": "DROPPED",
                "reason": "Tín hiệu cho cây nến này đã được phát trước đó.",
                "dedup_key": dedup_key
            })),
        );
    }

    // Bước 4: Macro Bias Filter (BULLISH/BEARISH/NEUTRAL/AUTO bằng EMA50)
    let effective_bias = if config.macro_bias.eq_ignore_ascii_case("AUTO") {
        let candles = state.candles.read().await;
        GoldMarketAnalyzer::detect_ema_trend_bias(&candles)
    } else {
        MacroBias::from_str_ci(&config.macro_bias)
    };

    if (is_buy && effective_bias == MacroBias::Bearish)
        || (!is_buy && effective_bias == MacroBias::Bullish)
    {
        let current_bias = effective_bias.as_str();
        warn!(
            "⚠️ [Macro Filter] Bỏ qua {:?} từ TradingView vì bias hiện tại là {}.",
            action_upper, current_bias
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "blocked_by_macro_bias",
                "action_taken": "DROPPED",
                "reason": format!("Macro Bias hiện tại là {}. Hệ thống chỉ cho phép lệnh thuận xu hướng.", current_bias),
                "current_bias": current_bias
            })),
        );
    }

    // Bước 5: Xây dựng tín hiệu từ TradingView Alert
    let entry_price = payload.price;
    let sl_buffer = config.gold_sl_buffer_points.max(4.0);

    let (stop_loss, reward_tp1, reward_tp2, tp1, tp2) = if is_buy {
        let sl = round2(entry_price - sl_buffer);
        let r_points = round2(entry_price - sl);
        let r_tp1 = round2(r_points * config.min_risk_reward);
        let r_tp2 = round2(r_points * 3.0);
        (
            sl,
            r_tp1,
            r_tp2,
            round2(entry_price + r_tp1),
            round2(entry_price + r_tp2),
        )
    } else {
        let sl = round2(entry_price + sl_buffer);
        let r_points = round2(sl - entry_price);
        let r_tp1 = round2(r_points * config.min_risk_reward);
        let r_tp2 = round2(r_points * 3.0);
        (
            sl,
            r_tp1,
            r_tp2,
            round2(entry_price - r_tp1),
            round2(entry_price - r_tp2),
        )
    };

    let risk_points = round2((entry_price - stop_loss).abs());
    let risk_reward_ratio = if risk_points > 0.0 {
        round2(reward_tp1 / risk_points)
    } else {
        0.0
    };

    let pos_calc = PositionSizer::calculate_lot_size(
        config.account_balance,
        config.risk_percentage,
        risk_points,
        reward_tp1,
        reward_tp2,
        entry_price,
        config.account_leverage,
    );

    let mut signal = Signal {
        id: format!("TV-{}-{}", now.timestamp_millis(), action_upper),
        symbol: payload.ticker.clone(),
        signal_type: if is_buy {
            SignalType::Buy
        } else {
            SignalType::Sell
        },
        timeframe: config.timeframe,
        entry_price,
        stop_loss,
        take_profit_1: tp1,
        take_profit_2: tp2,
        risk_points,
        reward_points_tp1: reward_tp1,
        reward_points_tp2: reward_tp2,
        risk_reward_ratio,
        volume_surge_ratio: 1.85,
        recommended_lot_size: pos_calc.lot_size,
        risk_amount_usd: pos_calc.risk_amount_usd,
        potential_profit_tp1_usd: pos_calc.potential_profit_tp1_usd,
        potential_profit_tp2_usd: pos_calc.potential_profit_tp2_usd,
        price_action_notes: payload.comment.unwrap_or_else(|| {
            format!("TradingView Webhook Alert kích hoạt tại mức giá ${:.2}", entry_price)
        }),
        volume_analysis_notes:
            "Xác nhận hấp thụ lực giá theo tín hiệu Pine Script từ TradingView".to_string(),
        full_rationale: format!(
            "TradingView Alert [{}] thực thi an toàn ngoài vùng cấm tin tức. Khối lượng tự động: {:.2} lot (Rủi ro: -${:.2}, TP1: +${:.2}). Tỷ lệ R:R chuẩn 1:{:.2}.",
            action_upper, pos_calc.lot_size, pos_calc.risk_amount_usd, pos_calc.potential_profit_tp1_usd, risk_reward_ratio
        ),
        status: SignalStatus::Active,
        created_at: now,
        telegram_message_id: None,
        dedup_key,
        candle_timestamp,
        is_simulated: false,
        confluence_score: 90,
        outcome_at: None,
    };

    // Bước 6: Bắn cảnh báo tức thì tới Telegram
    let telegram = state.telegram.read().await.clone();
    if let Ok(msg_id) = telegram.send_signal_alert(&signal).await {
        signal.telegram_message_id = Some(msg_id);
    }

    // Bước 7: Lưu vào bộ nhớ + đĩa
    state.signals.write().await.insert(0, signal.clone());
    if let Err(err) = persist_state(&state).await {
        warn!("⚠️ Không lưu được trạng thái sau webhook: {}", err);
    }

    (
        StatusCode::CREATED,
        Json(json!({
            "status": "signal_executed",
            "message": "Tín hiệu TradingView đã được phê duyệt qua Sniper Quota, Dedup & Kill Switch!",
            "signal": signal
        })),
    )
}

/// Lấy cấu hình tham số bot hiện tại
async fn get_config(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await;
    Json((*config).clone())
}

/// Cập nhật cấu hình bot từ Dashboard và lưu bền vững xuống đĩa.
async fn update_config(
    State(state): State<AppState>,
    Json(new_config): Json<BotConfig>,
) -> impl IntoResponse {
    let new_config = new_config.sanitized();
    {
        let mut config = state.config.write().await;
        *config = new_config.clone();
    }
    {
        let mut telegram = state.telegram.write().await;
        *telegram = TelegramNotifier::new(
            new_config.telegram_bot_token.clone(),
            new_config.telegram_chat_id.clone(),
        );
    }

    if let Err(err) = persist_state(&state).await {
        warn!("⚠️ Không lưu được config xuống đĩa: {}", err);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "message": format!("Cấu hình đã cập nhật trong RAM nhưng ghi đĩa thất bại: {}", err)
            })),
        );
    }

    info!("⚙️ Cấu hình Bot đã được cập nhật và lưu xuống đĩa!");
    (
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "message": "Đã lưu thông số cấu hình, quota Sniper và Telegram credentials.",
            "config": new_config
        })),
    )
}

#[derive(Debug, Default, Deserialize)]
struct CalculateLotRequest {
    #[serde(default)]
    account_balance: Option<f64>,
    #[serde(default)]
    risk_percentage: Option<f64>,
    #[serde(default)]
    sl_distance: Option<f64>,
    #[serde(default)]
    tp1_distance: Option<f64>,
    #[serde(default)]
    tp2_distance: Option<f64>,
    #[serde(default)]
    current_price: Option<f64>,
    #[serde(default)]
    leverage: Option<f64>,
}

/// Tính Lot Size trực tiếp từ PositionSizer (Rust) cho Dashboard.
async fn calculate_lot(
    State(state): State<AppState>,
    Json(payload): Json<CalculateLotRequest>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let default_price = state
        .candles
        .read()
        .await
        .last()
        .map(|candle| candle.close)
        .unwrap_or(2684.50);

    let balance = payload.account_balance.unwrap_or(config.account_balance);
    let risk_pct = payload.risk_percentage.unwrap_or(config.risk_percentage);
    let sl = payload.sl_distance.unwrap_or(9.5);
    let tp1 = payload.tp1_distance.unwrap_or(sl * 2.0);
    let tp2 = payload.tp2_distance.unwrap_or(sl * 3.0);
    let current_price = payload.current_price.unwrap_or(default_price);
    let leverage = payload.leverage.unwrap_or(config.account_leverage);

    let result =
        PositionSizer::calculate_lot_size(balance, risk_pct, sl, tp1, tp2, current_price, leverage);

    Json(result)
}

/// Lịch tin kinh tế + trạng thái Kill Switch đồng bộ với Frontend.
async fn get_news_calendar(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let now = Utc::now();

    let (events, active, active_event, reason) = {
        let nf = state.news_filter.read().await;
        let (active, active_event, reason) = nf.evaluate_news_status(now, &config);
        (nf.calendar_events.clone(), active, active_event, reason)
    };

    Json(json!({
        "events": events,
        "isKillSwitchActive": active,
        "activeEvent": active_event,
        "killSwitchReason": reason,
        "killSwitchEnabled": config.kill_switch_enabled,
        "macroBias": config.macro_bias
    }))
}

/// Lấy thông tin Macro Bias mới nhất trong ngày.
async fn get_latest_macro_bias(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let report = state.daily_report.read().await.clone();
    let now = Utc::now();

    let (is_kill_switch_active, active_event) = {
        let nf = state.news_filter.read().await;
        let (active, event, _) = nf.evaluate_news_status(now, &config);
        (active, event)
    };

    Json(json!({
        "macro_bias": config.macro_bias,
        "daily_report": report,
        "is_kill_switch_active": is_kill_switch_active,
        "active_blocking_event": active_event,
        "kill_switch_enabled": config.kill_switch_enabled
    }))
}

/// Kích hoạt phiên chạy Daily Cron thủ công (07:00 AM Scan).
async fn trigger_daily_macro_cron(State(state): State<AppState>) -> impl IntoResponse {
    info!("⚡ [Manual Trigger] Kích hoạt phiên quét Daily Cron 07:00 AM...");

    let ai_key = std::env::var("OPENROUTER_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .unwrap_or_default();
    let scheduler = DailyScheduler::new(ai_key, None);

    // 1. Quét lịch tin ForexFactory thật
    let events = scheduler.fetch_daily_economic_events().await;
    {
        let mut nf = state.news_filter.write().await;
        nf.update_events(events.clone());
    }

    // 2. Báo cáo vĩ mô định lượng EMA (miễn phí) hoặc AI nếu có key
    let candles = state.candles.read().await.clone();
    let mut report = if candles.is_empty() {
        scheduler.fetch_daily_macro_bias(2684.50).await
    } else {
        scheduler
            .fetch_daily_macro_bias_from_candles(&candles)
            .await
    };
    report.high_impact_events_today = events
        .iter()
        .filter(|event| event.impact == crate::news_filter::ImpactLevel::High)
        .take(6)
        .map(|event| {
            format!(
                "{} ({})",
                event.title,
                event.scheduled_time.format("%d/%m %H:%M UTC")
            )
        })
        .collect();

    // 3. Đồng bộ vào config & AppState rồi lưu đĩa
    {
        let mut cfg = state.config.write().await;
        cfg.macro_bias = report.macro_bias.as_str().to_string();
    }
    {
        let mut rep_lock = state.daily_report.write().await;
        *rep_lock = Some(report.clone());
    }
    if let Err(err) = persist_state(&state).await {
        warn!("⚠️ Không lưu được Daily Macro report: {}", err);
    }

    info!("✅ [Manual Trigger] Đã hoàn tất phiên quét Daily Cron!");

    (
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "message": "Đã chạy thành công phiên quét 07:00 AM: Cập nhật Lịch tin ForexFactory & Nhận định vĩ mô EMA!",
            "report": report,
            "events_synced": events.len()
        })),
    )
}

#[derive(Debug, Default, Deserialize)]
struct SimulateSetupRequest {
    #[serde(rename = "type", default)]
    setup_type: String,
}

/// Giả lập nhanh 1 setup (BUY_PINBAR / SELL_PINBAR / RANDOM_CANDLE).
/// Tín hiệu tạo ra được đánh dấu `is_simulated = true` nên KHÔNG tiêu hao quota 1 lệnh thật/ngày.
async fn simulate_setup(
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
            let support_price = supports
                .first()
                .map(|zone| zone.price)
                .unwrap_or(last_close - 6.0);
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
                timeframe: config.timeframe,
            }
        }
        "SELL_PINBAR" => {
            let resistance_price = resistances
                .first()
                .map(|zone| zone.price)
                .unwrap_or(last_close + 6.0);
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
                timeframe: config.timeframe,
            }
        }
    };

    candles_snapshot.push(new_candle.clone());
    if candles_snapshot.len() > 500 {
        candles_snapshot.remove(0);
    }
    {
        let mut candles = state.candles.write().await;
        *candles = candles_snapshot.clone();
    }
    evaluate_outcomes(&state).await;

    let detected_signal = GoldMarketAnalyzer::analyze_market(&candles_snapshot, &config);

    if let Some(mut signal) = detected_signal {
        signal.is_simulated = true;
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
            "message": "Nến mới đã thêm. Chưa hội tụ đủ điều kiện Pinbar + Volume tại vùng cản."
        })),
    )
}

#[derive(Debug, Default, Deserialize)]
struct TelegramTestPayload {
    #[serde(default)]
    token: Option<String>,
    #[serde(default, alias = "chatId")]
    chat_id: Option<String>,
}

/// Test gửi tin nhắn Telegram từ dashboard (dùng token/chatId gửi lên hoặc config).
async fn trigger_test_telegram(
    State(state): State<AppState>,
    payload: Option<Json<TelegramTestPayload>>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let request = payload.map(|Json(value)| value).unwrap_or_default();

    let token = request
        .token
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.telegram_bot_token.clone());
    let chat_id = request
        .chat_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.telegram_chat_id.clone());

    if token.trim().is_empty() || chat_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "message": "Vui lòng cung cấp Telegram Bot Token và Chat ID trước khi test!"
            })),
        );
    }

    let notifier = TelegramNotifier::new(token, chat_id);
    match notifier.send_test_ping().await {
        Ok(message) => (
            StatusCode::OK,
            Json(json!({ "status": "success", "message": message })),
        ),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "message": err })),
        ),
    }
}

async fn push_candle(state: &AppState, candle: Candle) {
    let mut candles = state.candles.write().await;
    candles.push(candle);
    if candles.len() > 500 {
        candles.remove(0);
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
