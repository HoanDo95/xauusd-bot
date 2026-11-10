use super::{
    ai_review_block_reason, apply_volume_delta_filter, attach_rsi_snapshot, evaluate_outcomes,
    htf_block_reason, persist_state, round2, rsi_block_reason, AppState,
};
use crate::analyzer::GoldMarketAnalyzer;
use crate::models::{
    BotConfig, Candle, EntryType, NewCandleWebhook, Signal, SignalStatus, SignalType, Timeframe,
    TradingViewWebhookPayload,
};
use crate::news_filter::MacroBias;
use crate::risk_manager::PositionSizer;
use crate::rsi::rsi_snapshot;
use crate::signal_validation::{validate_risk_budget, validate_signal_levels};
use crate::storage::PersistentStore;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde_json::json;
use tracing::{info, warn};

/// So sánh chuỗi theo thời gian hằng số để tránh rò rỉ thông tin qua timing.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }

    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

/// Passphrase rỗng nghĩa là webhook không bật kiểm tra.
fn passphrase_matches(provided: Option<&str>, expected: &str) -> bool {
    if expected.is_empty() {
        return true;
    }
    provided
        .map(|value| constant_time_eq(value, expected))
        .unwrap_or(false)
}

/// Webhook nhận nến mới từ sàn giao dịch (Binance / MetaTrader EA).
/// Enforce: Daily Quota -> Dedup -> News Kill Switch -> Analyzer.
pub(super) async fn receive_candle_webhook(
    State(state): State<AppState>,
    Json(payload): Json<NewCandleWebhook>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();

    if !passphrase_matches(payload.passphrase.as_deref(), &config.webhook_passphrase) {
        warn!("🔒 [Candle Webhook] Từ chối passphrase không hợp lệ.");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "unauthorized",
                "message": "Webhook passphrase không hợp lệ."
            })),
        );
    }

    let timeframe = match parse_webhook_timeframe(&payload.timeframe) {
        Some(timeframe) => timeframe,
        None => {
            warn!(
                "🛑 [Candle Webhook] Từ chối timeframe không hỗ trợ: {}",
                payload.timeframe
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "rejected_invalid_candle",
                    "reason": format!(
                        "timeframe '{}' không được hỗ trợ (chỉ nhận M15/H1/H4).",
                        payload.timeframe
                    )
                })),
            );
        }
    };

    let new_candle = Candle {
        // Missing timestamp = the candle that just closed, not the forming one.
        timestamp: payload
            .timestamp
            .unwrap_or_else(|| Utc::now().timestamp_millis() - timeframe_millis(timeframe)),
        open: payload.open,
        high: payload.high,
        low: payload.low,
        close: payload.close,
        volume: payload.volume,
        taker_buy_volume: 0.0,
        timeframe,
    };

    if let Err(reason) =
        validate_webhook_candle(&new_candle, &config, Utc::now().timestamp_millis())
    {
        warn!("🛑 [Candle Webhook] Từ chối nến không hợp lệ: {}", reason);
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "rejected_invalid_candle",
                "reason": reason
            })),
        );
    }

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

        // 4e. Cổng cuối: dedup/quota phải nguyên tử với Telegram + lưu tín hiệu.
        let _gate = state.signal_gate.lock().await;
        let signals_now = state.signals.read().await.clone();
        if PersistentStore::is_duplicate_candle(&signals_now, &signal.dedup_key) {
            warn!(
                "🛑 [Candle Webhook] Dedup sau phân tích — bỏ qua {}",
                signal.dedup_key
            );
            return (
                StatusCode::OK,
                Json(json!({ "status": "blocked_duplicate" })),
            );
        }
        if PersistentStore::has_reached_daily_quota(
            &signals_now,
            config.max_signals_per_day,
            Utc::now(),
        ) {
            warn!(
                "🛑 [Candle Webhook] Quota ngày đã đạt sau phân tích — bỏ qua {}",
                signal.dedup_key
            );
            return (
                StatusCode::OK,
                Json(json!({ "status": "blocked_by_quota" })),
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

    if !passphrase_matches(payload.passphrase.as_deref(), &config.webhook_passphrase) {
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
        let timeframe = match payload.timeframe.as_deref() {
            Some(raw) => match parse_webhook_timeframe(raw) {
                Some(timeframe) => timeframe,
                None => {
                    warn!(
                        "🛑 [TradingView Webhook] Từ chối timeframe không hỗ trợ: {}",
                        raw
                    );
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "status": "rejected_invalid_candle",
                            "reason": format!(
                                "timeframe '{}' không được hỗ trợ (chỉ nhận M15/H1/H4).",
                                raw
                            )
                        })),
                    );
                }
            },
            None => config.timeframe,
        };

        let candle = Candle {
            // Missing timestamp = the candle that just closed, not the forming one.
            timestamp: payload
                .timestamp
                .unwrap_or_else(|| now.timestamp_millis() - timeframe_millis(timeframe)),
            open: payload.open.unwrap_or(payload.price),
            high: payload.high.unwrap_or(payload.price + 1.0),
            low: payload.low.unwrap_or(payload.price - 1.0),
            close: payload.close.unwrap_or(payload.price),
            volume: payload.volume.unwrap_or(1500.0),
            taker_buy_volume: 0.0,
            timeframe,
        };

        if let Err(reason) = validate_webhook_candle(&candle, &config, now.timestamp_millis()) {
            warn!(
                "🛑 [TradingView Webhook] Từ chối nến không hợp lệ: {}",
                reason
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "rejected_invalid_candle",
                    "reason": reason
                })),
            );
        }

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

    // Giá XAU/USD phải nằm trong biên hợp lý trước khi dựng SL/TP.
    if !valid_gold_price(payload.price) {
        warn!(
            "🛑 [TradingView Webhook] Từ chối giá không hợp lệ: {}",
            payload.price
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "rejected_invalid_price",
                "reason": format!(
                    "Giá {} không hợp lệ cho XAU/USD (yêu cầu 500..10000).",
                    payload.price
                )
            })),
        );
    }

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
    // TP2 phải luôn nằm ngoài TP1, kể cả khi min_risk_reward = 5R.
    let tp2_r = tp2_r_multiple(config.min_risk_reward);

    let (stop_loss, reward_tp1, reward_tp2, tp1, tp2) = if is_buy {
        let sl = round2(entry_price - sl_buffer);
        let r_points = round2(entry_price - sl);
        let r_tp1 = round2(r_points * config.min_risk_reward);
        let r_tp2 = round2(r_points * tp2_r);
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
        let r_tp2 = round2(r_points * tp2_r);
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

    // Bước 6a: Chặn tín hiệu sai cấu trúc hoặc vượt ngân sách rủi ro trước khi
    // gắn filter, gửi Telegram hoặc lưu đĩa.
    if let Err(reason) = validate_signal_levels(
        signal.signal_type,
        signal.entry_price,
        signal.stop_loss,
        signal.take_profit_1,
        signal.take_profit_2,
        config.min_risk_reward,
    ) {
        warn!(
            "🛑 [TradingView Webhook] Từ chối tín hiệu không hợp lệ: {}",
            reason
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "rejected_invalid_signal",
                "reason": reason
            })),
        );
    }

    if let Err(reason) = validate_risk_budget(&config, &pos_calc) {
        warn!(
            "🛑 [TradingView Webhook] Từ chối tín hiệu vượt ngân sách rủi ro: {}",
            reason
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "rejected_invalid_signal",
                "reason": reason
            })),
        );
    }

    // Bước 6b: RSI M15 context — luôn gắn, kể cả khi filter tắt.
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

    // Bước 6c: Volume Delta/CVD tùy chọn (mặc định OFF) — chỉ tín hiệu thật.
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

    // Bước 6d: RSI filter tùy chọn (mặc định OFF) — chỉ tín hiệu thật.
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

    // Bước 6e: AI Risk Officer tùy chọn (chỉ veto/standby, không sửa SL/TP).
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

    // Bước 6f: Cổng cuối: dedup/quota phải nguyên tử với Telegram + lưu tín hiệu.
    let _gate = state.signal_gate.lock().await;
    let signals_now = state.signals.read().await.clone();
    if PersistentStore::is_duplicate_candle(&signals_now, &signal.dedup_key) {
        warn!(
            "🛑 [TradingView Webhook] Dedup sau phân tích — bỏ qua {}",
            signal.dedup_key
        );
        return (
            StatusCode::OK,
            Json(json!({ "status": "blocked_duplicate" })),
        );
    }
    if PersistentStore::has_reached_daily_quota(
        &signals_now,
        config.max_signals_per_day,
        Utc::now(),
    ) {
        warn!(
            "🛑 [TradingView Webhook] Quota ngày đã đạt sau phân tích — bỏ qua {}",
            signal.dedup_key
        );
        return (
            StatusCode::OK,
            Json(json!({ "status": "blocked_by_quota" })),
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

/// Giá vàng hợp lệ cho webhook: hữu hạn và nằm trong biên vận hành XAU/USD.
fn valid_gold_price(price: f64) -> bool {
    price.is_finite() && (500.0..=10_000.0).contains(&price)
}

/// Parse timeframe webhook (không phân biệt hoa/thường); chỉ nhận M15/H1/H4.
/// Chấp nhận thêm alias TradingView `{{interval}}`: 15 / 60 / 240.
fn parse_webhook_timeframe(value: &str) -> Option<Timeframe> {
    match value.trim().to_uppercase().as_str() {
        "M15" | "15M" | "15" => Some(Timeframe::M15),
        "H1" | "1H" | "60" => Some(Timeframe::H1),
        "H4" | "4H" | "240" => Some(Timeframe::H4),
        _ => None,
    }
}

fn timeframe_millis(timeframe: Timeframe) -> i64 {
    match timeframe {
        Timeframe::M15 => 15 * 60 * 1000,
        Timeframe::H1 => 60 * 60 * 1000,
        Timeframe::H4 => 4 * 60 * 60 * 1000,
    }
}

/// Kiểm tra nến webhook trước khi đưa vào buffer phân tích:
/// OHLC hợp lệ, cấu trúc high/low đúng, volume không âm, nến đã đóng và còn mới,
/// và timeframe phải khớp cấu hình bot.
fn validate_webhook_candle(candle: &Candle, config: &BotConfig, now_ms: i64) -> Result<(), String> {
    let prices = [candle.open, candle.high, candle.low, candle.close];
    if prices.iter().any(|price| !price.is_finite()) {
        return Err("OHLC phải là số hữu hạn.".to_string());
    }
    if let Some(price) = prices
        .iter()
        .find(|price| !(500.0..=10_000.0).contains(*price))
    {
        return Err(format!("OHLC {} nằm ngoài biên XAU/USD 500..10000.", price));
    }
    if candle.high < candle.open.max(candle.close).max(candle.low) {
        return Err("high phải >= max(open, close, low).".to_string());
    }
    if candle.low > candle.open.min(candle.close).min(candle.high) {
        return Err("low phải <= min(open, close, high).".to_string());
    }
    if candle.high < candle.low {
        return Err("high phải >= low.".to_string());
    }
    if !candle.volume.is_finite() || candle.volume < 0.0 {
        return Err("volume phải hữu hạn và >= 0.".to_string());
    }
    if candle.timestamp <= 0 {
        return Err("timestamp phải > 0.".to_string());
    }

    let expected_timeframe = config.timeframe.to_string();
    if !candle
        .timeframe
        .to_string()
        .eq_ignore_ascii_case(&expected_timeframe)
    {
        return Err(format!(
            "timeframe {} không khớp cấu hình {}.",
            candle.timeframe, expected_timeframe
        ));
    }

    let close_time = candle
        .timestamp
        .saturating_add(timeframe_millis(candle.timeframe));
    if close_time > now_ms.saturating_add(1000) {
        return Err("nến chưa đóng cửa (close_time nằm trong tương lai).".to_string());
    }

    let age_ms = now_ms - close_time;
    let max_age_ms = config.max_candle_age_secs.saturating_mul(1000);
    if age_ms > max_age_ms {
        return Err(format!(
            "nến quá cũ: {}s > {}s.",
            age_ms / 1000,
            config.max_candle_age_secs
        ));
    }

    Ok(())
}

/// Chỉ cho phép nối nến mới hơn cây cuối cùng; nến trùng/ngược thời gian bị bỏ.
fn should_append_candle(last_timestamp: Option<i64>, new_timestamp: i64) -> bool {
    match last_timestamp {
        Some(last) => new_timestamp > last,
        None => true,
    }
}

/// Bội số R cho TP2: tối thiểu 3R nhưng luôn vượt TP1 ít nhất 0.5R,
/// kể cả khi `min_risk_reward` được đẩy tới 5R.
fn tp2_r_multiple(min_risk_reward: f64) -> f64 {
    (min_risk_reward + 0.5).max(3.0)
}

async fn push_candle(state: &AppState, candle: Candle) {
    let mut candles = state.candles.write().await;
    let last_timestamp = candles.last().map(|last| last.timestamp);
    if !should_append_candle(last_timestamp, candle.timestamp) {
        warn!(
            "⚠️ [Candle Webhook] Bỏ qua nến trùng/ngược thời gian: {} <= {:?}.",
            candle.timestamp, last_timestamp
        );
        return;
    }
    candles.push(candle);
    if candles.len() > 500 {
        candles.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrase_matches_allows_empty_expected() {
        assert!(passphrase_matches(None, ""));
        assert!(passphrase_matches(Some("anything"), ""));
    }

    #[test]
    fn passphrase_matches_accepts_exact_value() {
        assert!(passphrase_matches(Some("long-secret"), "long-secret"));
    }

    #[test]
    fn passphrase_matches_rejects_mismatch() {
        assert!(!passphrase_matches(Some("wrong-secret"), "long-secret"));
    }

    #[test]
    fn passphrase_matches_rejects_missing_provided_value() {
        assert!(!passphrase_matches(None, "long-secret"));
    }

    #[test]
    fn passphrase_matches_rejects_different_lengths() {
        assert!(!passphrase_matches(Some("short"), "longer-secret"));
        assert!(!passphrase_matches(Some("longer-secret"), "short"));
    }

    #[test]
    fn constant_time_eq_matches_regular_equality() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn valid_gold_price_accepts_realistic_xauusd_range() {
        assert!(valid_gold_price(500.0));
        assert!(valid_gold_price(2400.0));
        assert!(valid_gold_price(10_000.0));
    }

    #[test]
    fn valid_gold_price_rejects_outliers_and_non_finite_values() {
        for bad in [
            0.0,
            -1.0,
            499.99,
            10_000.01,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            assert!(!valid_gold_price(bad), "{bad} must be rejected");
        }
    }

    fn candle_for(now_ms: i64, timeframe: Timeframe) -> Candle {
        Candle {
            timestamp: now_ms - timeframe_millis(timeframe),
            open: 2000.0,
            high: 2005.0,
            low: 1995.0,
            close: 2002.0,
            volume: 100.0,
            taker_buy_volume: 0.0,
            timeframe,
        }
    }

    #[test]
    fn validate_webhook_candle_accepts_closed_fresh_m15() {
        let now_ms = 1_700_000_000_000;
        let candle = candle_for(now_ms, Timeframe::M15);
        assert!(validate_webhook_candle(&candle, &BotConfig::default(), now_ms).is_ok());
    }

    #[test]
    fn parse_webhook_timeframe_accepts_case_and_tradingview_aliases() {
        for raw in ["M15", "m15", "15", "15m"] {
            assert_eq!(parse_webhook_timeframe(raw), Some(Timeframe::M15), "{raw}");
        }
        for raw in ["H1", "h1", "60", "1h"] {
            assert_eq!(parse_webhook_timeframe(raw), Some(Timeframe::H1), "{raw}");
        }
        for raw in ["H4", "h4", "240", "4h"] {
            assert_eq!(parse_webhook_timeframe(raw), Some(Timeframe::H4), "{raw}");
        }
        assert_eq!(parse_webhook_timeframe("W1"), None);
        assert_eq!(parse_webhook_timeframe(""), None);
    }

    #[test]
    fn validate_webhook_candle_rejects_high_below_low() {
        let now_ms = 1_700_000_000_000;
        let mut candle = candle_for(now_ms, Timeframe::M15);
        candle.high = 1990.0;
        candle.low = 2000.0;
        candle.open = 1995.0;
        candle.close = 1996.0;

        let error = validate_webhook_candle(&candle, &BotConfig::default(), now_ms)
            .expect_err("high < low must fail");
        assert!(error.contains("high"), "{error}");
    }

    #[test]
    fn validate_webhook_candle_rejects_non_finite_ohlc() {
        let now_ms = 1_700_000_000_000;
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut candle = candle_for(now_ms, Timeframe::M15);
            candle.open = bad;
            assert!(
                validate_webhook_candle(&candle, &BotConfig::default(), now_ms).is_err(),
                "open {bad} must be rejected"
            );
        }
    }

    #[test]
    fn validate_webhook_candle_rejects_stale_candle() {
        let now_ms = 1_700_000_000_000;
        let mut candle = candle_for(now_ms, Timeframe::M15);
        // Đóng cửa cách đây 5 phút > max_candle_age_secs mặc định 120s.
        candle.timestamp = now_ms - timeframe_millis(Timeframe::M15) - 5 * 60 * 1000;

        let error = validate_webhook_candle(&candle, &BotConfig::default(), now_ms)
            .expect_err("stale candle must fail");
        assert!(error.contains("cũ"), "{error}");
    }

    #[test]
    fn validate_webhook_candle_rejects_future_or_unclosed_candle() {
        let now_ms = 1_700_000_000_000;
        let mut candle = candle_for(now_ms, Timeframe::M15);
        // Nến mở 5 phút trước, còn 10 phút nữa mới đóng.
        candle.timestamp = now_ms - 5 * 60 * 1000;

        let error = validate_webhook_candle(&candle, &BotConfig::default(), now_ms)
            .expect_err("unclosed candle must fail");
        assert!(error.contains("chưa đóng"), "{error}");
    }

    #[test]
    fn validate_webhook_candle_rejects_timeframe_mismatch() {
        let now_ms = 1_700_000_000_000;
        let candle = candle_for(now_ms, Timeframe::H1);

        let error = validate_webhook_candle(&candle, &BotConfig::default(), now_ms)
            .expect_err("H1 candle must fail for M15 config");
        assert!(error.contains("timeframe"), "{error}");
    }

    #[test]
    fn validate_webhook_candle_rejects_negative_volume() {
        let now_ms = 1_700_000_000_000;
        let mut candle = candle_for(now_ms, Timeframe::M15);
        candle.volume = -1.0;

        let error = validate_webhook_candle(&candle, &BotConfig::default(), now_ms)
            .expect_err("negative volume must fail");
        assert!(error.contains("volume"), "{error}");
    }

    #[test]
    fn validate_webhook_candle_rejects_zero_timestamp() {
        let now_ms = 1_700_000_000_000;
        let mut candle = candle_for(now_ms, Timeframe::M15);
        candle.timestamp = 0;

        let error = validate_webhook_candle(&candle, &BotConfig::default(), now_ms)
            .expect_err("zero timestamp must fail");
        assert!(error.contains("timestamp"), "{error}");
    }

    #[test]
    fn push_candle_guard_rejects_duplicates_and_out_of_order() {
        assert!(should_append_candle(None, 1_700_000_000_000));
        assert!(should_append_candle(
            Some(1_700_000_000_000),
            1_700_000_900_000
        ));
        assert!(!should_append_candle(
            Some(1_700_000_000_000),
            1_700_000_000_000
        ));
        assert!(!should_append_candle(
            Some(1_700_000_000_000),
            1_699_999_100_000
        ));
    }

    #[test]
    fn tp2_multiple_is_at_least_three_r() {
        assert!((tp2_r_multiple(1.5) - 3.0).abs() < 1e-9);
        assert!((tp2_r_multiple(2.0) - 3.0).abs() < 1e-9);
        assert!((tp2_r_multiple(3.0) - 3.5).abs() < 1e-9);
    }

    #[test]
    fn tp2_multiple_stays_beyond_tp1_at_five_r() {
        let entry = 2400.0;
        let risk = 10.0;
        let tp1 = entry + risk * 5.0;
        let tp2 = entry + risk * tp2_r_multiple(5.0);

        assert!((tp2_r_multiple(5.0) - 5.5).abs() < 1e-9);
        assert!(tp2 > tp1 + 0.01);
    }
}
