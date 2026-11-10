use super::{
    ai_review_block_reason, apply_volume_delta_filter, attach_rsi_snapshot, evaluate_outcomes,
    htf_block_reason, persist_state, round2, rsi_block_reason, AppState,
};
use crate::analyzer::GoldMarketAnalyzer;
use crate::models::{
    Candle, EntryType, NewCandleWebhook, Signal, SignalStatus, SignalType, Timeframe,
    TradingViewWebhookPayload,
};
use crate::news_filter::MacroBias;
use crate::risk_manager::PositionSizer;
use crate::rsi::rsi_snapshot;
use crate::storage::PersistentStore;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde_json::json;
use tracing::{info, warn};

/// Webhook nhận nến mới từ sàn giao dịch (Binance / MetaTrader EA).
/// Enforce: Daily Quota -> Dedup -> News Kill Switch -> Analyzer.
pub(super) async fn receive_candle_webhook(
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
        taker_buy_volume: 0.0,
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

        // 4a. RSI M15 context — luôn gắn, kể cả khi filter tắt.
        let rsi = rsi_snapshot(
            &candles_snapshot,
            config.rsi_period,
            config.rsi_overbought,
            config.rsi_oversold,
            config.rsi_divergence_lookback,
            config.rsi_pivot_k,
        );
        attach_rsi_snapshot(&mut signal, &rsi);

        if let Some(reason) = htf_block_reason(&state, &config, signal.signal_type.as_str()).await {
            warn!(
                "🛑 [Candle Webhook] Chặn {} do bộ lọc H4: {}",
                signal.signal_type, reason
            );
            return (
                StatusCode::OK,
                Json(json!({
                    "status": "blocked_by_htf",
                    "reason": reason
                })),
            );
        }

        if let Some(reason) = apply_volume_delta_filter(&config, &mut signal, &candles_snapshot) {
            warn!(
                "🛑 [Candle Webhook] Volume Delta chặn {}: {}",
                signal.signal_type, reason
            );
            return (
                StatusCode::OK,
                Json(json!({
                    "status": "blocked_by_volume_delta",
                    "reason": reason
                })),
            );
        }

        if let Some(reason) = rsi_block_reason(&config, &signal, &rsi) {
            warn!(
                "🛑 [Candle Webhook] RSI filter chặn {}: {}",
                signal.signal_type, reason
            );
            return (
                StatusCode::OK,
                Json(json!({
                    "status": "blocked_by_rsi",
                    "reason": reason
                })),
            );
        }

        if let Some(reason) = ai_review_block_reason(&state, &config, &mut signal).await {
            warn!(
                "🤖 [Candle Webhook] AI Risk Officer chặn {}: {}",
                signal.id, reason
            );
            return (
                StatusCode::OK,
                Json(json!({
                    "status": "blocked_by_ai_review",
                    "action_taken": "DROPPED",
                    "reason": reason
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
pub(super) async fn receive_tradingview_webhook(
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
            taker_buy_volume: 0.0,
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

    // Bước 5: Bộ lọc H4 tùy chọn (mặc định OFF) — chặn tín hiệu ngược xu hướng H4.
    if let Some(reason) =
        htf_block_reason(&state, &config, if is_buy { "BUY" } else { "SELL" }).await
    {
        warn!(
            "🛑 [TradingView Webhook] Chặn {} do bộ lọc H4: {}",
            action_upper, reason
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "blocked_by_htf",
                "action_taken": "DROPPED",
                "reason": reason
            })),
        );
    }

    // Bước 6: Xây dựng tín hiệu từ TradingView Alert
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
        entry_type: EntryType::Market,
        candle_close_price: entry_price,
        live_market_price: Some(entry_price),
        retest_limit_price: None,
        data_source: config.data_source.clone(),
        is_entry_filled: true,
        ai_verdict: None,
        ai_confidence: None,
        ai_reason: None,
        delta_ratio: None,
        cvd: None,
        delta: None,
        rsi_value: None,
        rsi_state: None,
        rsi_divergence: None,
    };

    // Bước 6a: RSI M15 context — luôn gắn, kể cả khi filter tắt.
    let candles_snapshot = state.candles.read().await.clone();
    let rsi = rsi_snapshot(
        &candles_snapshot,
        config.rsi_period,
        config.rsi_overbought,
        config.rsi_oversold,
        config.rsi_divergence_lookback,
        config.rsi_pivot_k,
    );
    attach_rsi_snapshot(&mut signal, &rsi);

    // Bước 6b: Volume Delta/CVD tùy chọn (mặc định OFF) — chỉ tín hiệu thật.
    if let Some(reason) = apply_volume_delta_filter(&config, &mut signal, &candles_snapshot) {
        warn!(
            "🛑 [TradingView Webhook] Volume Delta chặn {}: {}",
            action_upper, reason
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "blocked_by_volume_delta",
                "action_taken": "DROPPED",
                "reason": reason
            })),
        );
    }

    // Bước 6c: RSI filter tùy chọn (mặc định OFF) — chỉ tín hiệu thật.
    if let Some(reason) = rsi_block_reason(&config, &signal, &rsi) {
        warn!(
            "🛑 [TradingView Webhook] RSI filter chặn {}: {}",
            action_upper, reason
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "blocked_by_rsi",
                "action_taken": "DROPPED",
                "reason": reason
            })),
        );
    }

    // Bước 6d: AI Risk Officer tùy chọn (chỉ veto/standby, không sửa SL/TP).
    if let Some(reason) = ai_review_block_reason(&state, &config, &mut signal).await {
        warn!(
            "🤖 [TradingView Webhook] AI Risk Officer chặn {}: {}",
            signal.id, reason
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "blocked_by_ai_review",
                "action_taken": "DROPPED",
                "reason": reason
            })),
        );
    }

    // Bước 7: Bắn cảnh báo tức thì tới Telegram
    let telegram = state.telegram.read().await.clone();
    if let Ok(msg_id) = telegram.send_signal_alert(&signal).await {
        signal.telegram_message_id = Some(msg_id);
    }

    // Bước 8: Lưu vào bộ nhớ + đĩa
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

async fn push_candle(state: &AppState, candle: Candle) {
    let mut candles = state.candles.write().await;
    candles.push(candle);
    if candles.len() > 500 {
        candles.remove(0);
    }
}
