use crate::analyzer::GoldMarketAnalyzer;
use crate::api::{persist_state, AppState};
use crate::market_context::{
    build_market_context, htf_allows_signal, shift_candles_prices, MarketContext,
};
use crate::market_feed::{MarketDataFeed, MarketFeedSnapshot};
use crate::models::{BotConfig, Candle, Timeframe};
use crate::news_filter::ImpactLevel;
use crate::rsi::{divergence_label, rsi_allows_signal, rsi_snapshot};
use crate::scheduler::DailyScheduler;
use crate::session_alerts::evaluate_and_dispatch;
use crate::signal_review::{review_blocks_signal, review_signal};
use crate::storage::PersistentStore;
use crate::tracker::evaluate_active_signals;
use crate::volume_delta::delta_allows_signal;
use chrono::Utc;
use std::time::Instant;
use tracing::{info, warn};

/// Cache spot XAU/USD 300s (~288 calls/day), an toàn cho TwelveData free tier.
const SPOT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(300);

async fn resolve_spot_price(
    market_feed: &MarketDataFeed,
    cache: &mut Option<(f64, Instant)>,
) -> Option<f64> {
    if let Some((price, fetched_at)) = *cache {
        if fetched_at.elapsed() < SPOT_CACHE_TTL {
            return Some(price);
        }
    }

    let price = market_feed.fetch_spot_xau_usd().await?;
    *cache = Some((price, Instant::now()));
    Some(price)
}

/// PAXG basis = spot XAU/USD − (live PAXG − broker offset thủ công).
///
/// Chỉ chấp nhận basis hữu hạn và nằm trong biên hợp lý để một lần spot lỗi
/// (giá 0/NaN/outlier) không thể kéo toàn bộ nến PAXG lệch hàng trăm USD.
fn compute_spot_basis(
    snapshot: &MarketFeedSnapshot,
    spot_price: f64,
    broker_price_offset: f64,
) -> Option<f64> {
    let raw_live = snapshot
        .live_price
        .or_else(|| snapshot.candles.last().map(|candle| candle.close))?;

    if !spot_price.is_finite()
        || spot_price <= 0.0
        || !(500.0..=10_000.0).contains(&spot_price)
        || !raw_live.is_finite()
    {
        return None;
    }

    let basis = spot_price - (raw_live - broker_price_offset);
    if !basis.is_finite() || basis.abs() > (spot_price * 0.02).max(50.0) {
        return None;
    }

    Some(basis.clamp(-100.0, 100.0))
}

/// Căn snapshot Binance PAXG về spot XAU/USD. Trả (spot, basis) nếu áp dụng thành công.
pub async fn apply_spot_basis(
    snapshot: &mut MarketFeedSnapshot,
    config: &BotConfig,
    market_feed: &MarketDataFeed,
    cache: &mut Option<(f64, Instant)>,
) -> Option<(f64, f64)> {
    if !config.use_auto_spot_basis || snapshot.source != "BINANCE_PAXG" {
        return None;
    }

    let spot_price = resolve_spot_price(market_feed, cache).await?;
    let basis = compute_spot_basis(snapshot, spot_price, config.broker_price_offset)?;
    MarketDataFeed::shift_snapshot_prices(snapshot, basis);
    info!("🌐 Spot basis: spot={:.2}, basis={:+.2}", spot_price, basis);
    Some((spot_price, basis))
}

/// Migration: build cũ tự động đổi `data_source` sang TWELVEDATA khi có key.
/// TwelveData `time_series` không có volume nên OHLCV phải quay lại Binance PAXG;
/// TwelveData chỉ còn là giá spot tham chiếu.
pub fn migrate_ohlcv_source_to_binance(config: &mut BotConfig) {
    if config.data_source.eq_ignore_ascii_case("TWELVEDATA") {
        warn!(
            "⚠️ data_source=TWELVEDATA từ cấu hình cũ không có volume — chuyển OHLCV sang BINANCE_PAXG (TwelveData chỉ dùng spot basis)."
        );
        config.data_source = "BINANCE_PAXG".to_string();
    }
}

/// Nhãn nguồn giá cho panel "view toàn cảnh" (HTF luôn lấy từ Binance PAXG).
fn htf_price_source(spot_basis: Option<f64>) -> String {
    if spot_basis.is_some() {
        "BINANCE_PAXG+TWELVEDATA_SPOT".to_string()
    } else {
        "BINANCE_PAXG".to_string()
    }
}

/// Lượng dịch giá cho HTF: ưu tiên spot basis (đã bao gồm broker offset khi
/// auto-basis bật), nếu không có thì dùng broker offset thủ công để H4/D1/H1
/// thẳng hàng với nến M15. Caller chỉ áp dụng khi giá trị hữu hạn và khác 0.
pub fn htf_shift_amount(spot_basis: Option<f64>, broker_price_offset: f64) -> f64 {
    spot_basis.unwrap_or(broker_price_offset)
}

/// Fetch H4/D1/H1 đã đóng cửa và dựng MarketContext. Trả None nếu thiếu H4/D1.
/// Kèm theo nến H4/D1 (đã căn basis) để dashboard vẽ "view toàn cảnh".
pub async fn fetch_market_context(
    market_feed: &MarketDataFeed,
    spot_basis: Option<f64>,
    broker_price_offset: f64,
) -> Option<(MarketContext, Vec<Candle>, Vec<Candle>)> {
    let (mut h4, mut d1, mut h1) = tokio::join!(
        market_feed.fetch_htf_candles(Timeframe::H4, 300),
        market_feed.fetch_daily_htf_candles(300),
        market_feed.fetch_htf_candles(Timeframe::H1, 300),
    );

    let shift = htf_shift_amount(spot_basis, broker_price_offset);
    if shift.is_finite() && shift != 0.0 {
        shift_candles_prices(&mut h4, shift);
        shift_candles_prices(&mut d1, shift);
        shift_candles_prices(&mut h1, shift);
    }

    if h4.is_empty() || d1.is_empty() {
        warn!(
            "⚠️ [HTF Context] Thiếu nến H4 ({}) hoặc D1 ({}), giữ bối cảnh cũ.",
            h4.len(),
            d1.len()
        );
        return None;
    }
    if h1.is_empty() {
        warn!("⚠️ [HTF Context] Không có nến H1, panel H1 sẽ trống.");
    }

    Some((
        build_market_context(&d1, &h4, &h1, &htf_price_source(spot_basis), spot_basis),
        h4,
        d1,
    ))
}

/// Background Scanner 24/7 — quét mỗi 60 giây.
///
/// `initial_spot_cache` giữ lại kết quả spot/basis vừa fetch lúc bootstrap để
/// tick đầu tiên không gọi thêm TwelveData ngoài cửa sổ cache 300s.
pub fn spawn_scanner(state: AppState, initial_spot_cache: Option<(f64, Instant)>) {
    tokio::spawn(async move {
        info!("⏱️ Sniper Worker 24/7 đã khởi chạy (chu kỳ 60 giây)...");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        let mut cached_spot = initial_spot_cache;

        loop {
            interval.tick().await;
            *state.last_scan_at.write().await = Some(Utc::now());

            // 6.1 Luôn refresh nến thật đã đóng cửa (kể cả khi bot tạm dừng).
            let config = state.config.read().await.clone();
            let mut snapshot = state
                .market_feed
                .fetch_market_snapshot(
                    config.timeframe,
                    300,
                    &config.data_source,
                    config.broker_price_offset,
                )
                .await;

            // 6.1b Căn PAXG về spot XAU/USD trước khi tracker/analyzer nhìn thấy giá.
            let applied_spot =
                apply_spot_basis(&mut snapshot, &config, &state.market_feed, &mut cached_spot)
                    .await;
            let (scan_spot_price, scan_spot_basis) = match applied_spot {
                Some((spot, basis)) => {
                    *state.spot_basis_at.write().await = Some(Utc::now());
                    (Some(spot), Some(basis))
                }
                None => {
                    // Spot lỗi nhưng vẫn có PAXG thật: dùng lại basis tốt gần nhất
                    // (tối đa 15 phút) thay vì phân tích giá PAXG thô.
                    if config.use_auto_spot_basis && snapshot.source == "BINANCE_PAXG" {
                        let existing_basis = *state.spot_basis.read().await;
                        let basis_at = *state.spot_basis_at.read().await;
                        let basis_age_secs = basis_at.map(|at| (Utc::now() - at).num_seconds());
                        let basis_is_fresh = basis_age_secs.map(|age| age <= 900).unwrap_or(false);
                        match existing_basis.filter(|basis| basis.is_finite() && basis_is_fresh) {
                            Some(basis) => {
                                MarketDataFeed::shift_snapshot_prices(&mut snapshot, basis);
                                let existing_spot = *state.spot_price.read().await;
                                warn!(
                                    "⚠️ [Scanner] Không lấy được spot mới — dùng lại basis {:+} (tuổi {}s).",
                                    basis,
                                    basis_age_secs.unwrap_or(0)
                                );
                                (existing_spot, Some(basis))
                            }
                            None => {
                                *state.active_data_source.write().await =
                                    "SPOT_BASIS_UNAVAILABLE".to_string();
                                warn!(
                                    "⚠️ [Scanner] Không có spot basis hợp lệ (<15 phút) — bỏ qua tracker & phân tích, không dùng PAXG thô."
                                );
                                continue;
                            }
                        }
                    } else {
                        (None, None)
                    }
                }
            };
            *state.spot_price.write().await = scan_spot_price;
            *state.spot_basis.write().await = scan_spot_basis;

            // Không bao giờ phân tích/tracker trên nến giả fallback.
            if snapshot.source == "FALLBACK_SEED" {
                *state.active_data_source.write().await = "FALLBACK_SEED".to_string();
                warn!(
                    "⚠️ [Scanner] Cả Binance và TwelveData đều lỗi — giữ nguyên nến cũ, bỏ qua tracker & phân tích."
                );
                continue;
            }

            let fresh_candles = snapshot.candles;
            if !fresh_candles.is_empty() {
                // Cập nhật TP1/TP2/SL cho các tín hiệu đang mở trước khi ghi đè nến.
                let updates = {
                    let mut signals = state.signals.write().await;
                    evaluate_active_signals(&mut signals, &fresh_candles)
                };
                *state.candles.write().await = fresh_candles;
                *state.live_price.write().await = snapshot.live_price;
                *state.active_data_source.write().await = snapshot.source.clone();
                if !updates.is_empty() {
                    info!("📈 Đã cập nhật {} tín hiệu theo TP/SL", updates.len());
                    for update in updates.iter().filter(|signal| !signal.is_simulated) {
                        let telegram = state.telegram.read().await.clone();
                        if let Err(err) = telegram.send_outcome_alert(update).await {
                            warn!("⚠️ Không gửi được Telegram outcome: {}", err);
                        }
                    }
                    if let Err(err) = persist_state(&state).await {
                        warn!("⚠️ Không lưu được kết quả TP/SL: {}", err);
                    }
                }
            }

            // 6.2 Bot đang hoạt động?
            if !config.is_active {
                continue;
            }

            let now = Utc::now();
            let signals_snapshot = state.signals.read().await.clone();

            // 6.3 Daily Sniper Quota: tối đa `max_signals_per_day` lệnh THẬT/ngày.
            if PersistentStore::has_reached_daily_quota(
                &signals_snapshot,
                config.max_signals_per_day,
                now,
            ) {
                continue;
            }

            // 6.4 Chỉ quét trong khung giờ thanh khoản Âu-Mỹ (13:00-23:00 GMT+7).
            if !PersistentStore::is_within_golden_session(now, config.session_filter_enabled) {
                continue;
            }

            // 6.5 News Kill Switch: không vào lệnh quanh tin High Impact USD.
            let is_market_safe = state
                .news_filter
                .read()
                .await
                .check_kill_switch(now, &config);
            if !is_market_safe {
                continue;
            }

            // 6.6 Dedup: cùng một cây nến chỉ được đánh giá/phát tín hiệu tối đa 1 lần.
            let candles = state.candles.read().await.clone();
            let latest_timestamp = match candles.last() {
                Some(candle) => candle.timestamp,
                None => continue,
            };
            let candle_dedup_key = format!(
                "{}_{}_{}",
                config.symbol, config.timeframe, latest_timestamp
            );
            if PersistentStore::is_duplicate_candle(&signals_snapshot, &candle_dedup_key) {
                continue;
            }

            // 6.7 Phân tích và phát tín hiệu nếu đủ 5 tầng lọc.
            let live_price = *state.live_price.read().await;
            if let Some(mut signal) = GoldMarketAnalyzer::analyze_market_with_live_price(
                &candles,
                &config,
                live_price,
                Utc::now().timestamp_millis(),
            ) {
                if PersistentStore::is_duplicate_candle(&signals_snapshot, &signal.dedup_key) {
                    continue;
                }

                // 6.7a RSI M15 (advisory/context) — luôn gắn vào tín hiệu.
                let rsi = rsi_snapshot(
                    &candles,
                    config.rsi_period,
                    config.rsi_overbought,
                    config.rsi_oversold,
                    config.rsi_divergence_lookback,
                    config.rsi_pivot_k,
                );
                signal.rsi_value = rsi.value;
                signal.rsi_state = Some(rsi.state.clone());
                signal.rsi_divergence = Some(divergence_label(rsi.divergence));

                // 6.7b Bộ lọc H4/D1 tùy chọn (mặc định OFF).
                if config.htf_h4_filter_enabled || config.htf_d1_filter_enabled {
                    let context = state.market_context.read().await;
                    if let Some(context) = context.as_ref() {
                        let (allowed, reason) = htf_allows_signal(
                            signal.signal_type.as_str(),
                            context,
                            config.htf_h4_filter_enabled,
                            config.htf_d1_filter_enabled,
                        );
                        if !allowed {
                            warn!(
                                "🛑 [HTF Filter] Bỏ qua {} {}: {}",
                                signal.signal_type, signal.symbol, reason
                            );
                            continue;
                        }
                    }
                }

                // 6.7c Volume Delta/CVD tùy chọn (mặc định OFF).
                // Chỉ áp dụng cho tín hiệu thật; simulation bỏ qua hoàn toàn.
                if config.volume_delta_filter_enabled && !signal.is_simulated {
                    let (allowed, reason, snapshot) =
                        delta_allows_signal(signal.signal_type.as_str(), &candles, &config);
                    if !allowed {
                        warn!(
                            "🛑 [Volume Delta] Bỏ qua {} {}: {}",
                            signal.signal_type, signal.symbol, reason
                        );
                        continue;
                    }
                    signal.delta_ratio = Some(snapshot.delta_ratio);
                    signal.cvd = Some(snapshot.cvd);
                    signal.delta = Some(snapshot.delta);
                    info!("📊 [Volume Delta] {} — {}", signal.id, reason);
                }

                // 6.7d RSI filter tùy chọn (mặc định OFF).
                // Chỉ áp dụng cho tín hiệu thật; simulation bỏ qua hoàn toàn.
                if config.rsi_filter_enabled && !signal.is_simulated {
                    let (allowed, reason) =
                        rsi_allows_signal(signal.signal_type.as_str(), &rsi, &config);
                    if !allowed {
                        warn!(
                            "🛑 [RSI Filter] Bỏ qua {} {}: {}",
                            signal.signal_type, signal.symbol, reason
                        );
                        continue;
                    }
                    info!("📐 [RSI Filter] {} — {}", signal.id, reason);
                }

                // 6.7e AI Chief Risk Officer tùy chọn (mặc định OFF).
                // AI chỉ được veto/standby; không bao giờ sửa SL/TP live.
                if config.ai_signal_review_enabled {
                    let mut context = state.market_context.read().await.clone();
                    if let Some(context) = context.as_mut() {
                        context.rsi_m15 = Some(rsi.clone());
                    }
                    let events = state.news_filter.read().await.calendar_events.clone();

                    match review_signal(&signal, context.as_ref(), &config, &events).await {
                        Some(review) => {
                            signal.ai_verdict = Some(review.action.clone());
                            signal.ai_confidence = Some(review.confidence_score);
                            signal.ai_reason = Some(review.reason.clone());

                            let (blocked, reason) = review_blocks_signal(
                                &signal,
                                &review,
                                config.ai_signal_review_min_confidence,
                            );
                            if blocked {
                                warn!(
                                    "🤖 [AI Risk Officer] Bỏ qua {} {}: {}",
                                    signal.signal_type, signal.symbol, reason
                                );
                                continue;
                            }
                            info!(
                                "✅ [AI Risk Officer] Cho qua {} {} — {}",
                                signal.signal_type, signal.symbol, reason
                            );
                        }
                        None => {
                            if config.ai_signal_review_fail_open {
                                warn!(
                                    "⚠️ [AI Risk Officer] Không có review cho {} {} — fail-open, cho tín hiệu qua.",
                                    signal.signal_type, signal.symbol
                                );
                            } else {
                                warn!(
                                    "🛑 [AI Risk Officer] Không có review cho {} {} — fail-closed, bỏ qua tín hiệu.",
                                    signal.signal_type, signal.symbol
                                );
                                continue;
                            }
                        }
                    }
                }

                // 6.7f Cổng cuối: dedup/quota phải được kiểm tra nguyên tử với
                // bước gửi Telegram + lưu tín hiệu. Giữ khóa tới hết vòng lặp.
                let _gate = state.signal_gate.lock().await;
                let signals_now = state.signals.read().await.clone();
                if PersistentStore::is_duplicate_candle(&signals_now, &signal.dedup_key) {
                    warn!(
                        "🛑 [Scanner] Dedup sau phân tích — bỏ qua {} {}",
                        signal.signal_type, signal.symbol
                    );
                    continue;
                }
                if PersistentStore::has_reached_daily_quota(
                    &signals_now,
                    config.max_signals_per_day,
                    Utc::now(),
                ) {
                    warn!(
                        "🛑 [Scanner] Quota ngày đã đạt sau phân tích — bỏ qua {} {}",
                        signal.signal_type, signal.symbol
                    );
                    continue;
                }

                info!(
                    "🚨 [Sniper Scanner] Tín hiệu {} {} | Entry={} | SL={} | Confluence={}/100",
                    signal.signal_type,
                    signal.symbol,
                    signal.entry_price,
                    signal.stop_loss,
                    signal.confluence_score
                );

                let telegram = state.telegram.read().await.clone();
                if let Ok(msg_id) = telegram.send_signal_alert(&signal).await {
                    signal.telegram_message_id = Some(msg_id);
                }

                signal.is_simulated = false;
                state.signals.write().await.insert(0, signal);

                if let Err(err) = persist_state(&state).await {
                    warn!("⚠️ Không lưu được trạng thái sau tín hiệu: {}", err);
                }
            }
        }
    });
}

/// HTF Context Worker — refresh D1/H4/H1 mỗi 15 phút (read-only, không vào lệnh).
pub fn spawn_htf_context(state: AppState) {
    tokio::spawn(async move {
        info!("🧭 [HTF Context] Worker làm mới D1/H4/H1 mỗi 15 phút...");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15 * 60)).await;
            let config = state.config.read().await.clone();
            let spot_basis = *state.spot_basis.read().await;
            if let Some((context, h4, d1)) =
                fetch_market_context(&state.market_feed, spot_basis, config.broker_price_offset)
                    .await
            {
                *state.market_context.write().await = Some(context);
                *state.htf_h4_candles.write().await = h4;
                *state.htf_d1_candles.write().await = d1;
                info!("✅ [HTF Context] Đã cập nhật bối cảnh D1/H4/H1.");
            }
        }
    });
}

/// Daily Cron 07:00 AM — đồng bộ ForexFactory + báo cáo EMA.
pub fn spawn_daily_scheduler(state: AppState, scheduler: DailyScheduler) {
    tokio::spawn(async move {
        info!("⏰ [Daily Scheduler] Khởi động Cron Job 07:00 AM mỗi ngày...");
        loop {
            let sleep_seconds = DailyScheduler::seconds_until_next_seven_am();
            info!(
                "⏳ [Daily Scheduler] Lần chạy tiếp theo sau {} giây.",
                sleep_seconds
            );
            tokio::time::sleep(std::time::Duration::from_secs(sleep_seconds)).await;

            let events = scheduler.fetch_daily_economic_events().await;
            {
                let mut nf = state.news_filter.write().await;
                nf.update_events(events.clone());
            }

            let candles = state.candles.read().await.clone();
            let mut report = scheduler
                .fetch_daily_macro_bias_from_candles(&candles)
                .await;
            report.high_impact_events_today = events
                .iter()
                .filter(|event| event.impact == ImpactLevel::High)
                .take(6)
                .map(|event| {
                    format!(
                        "{} ({})",
                        event.title,
                        event.scheduled_time.format("%d/%m %H:%M UTC")
                    )
                })
                .collect();

            // Bias của báo cáo chỉ mang tính tham khảo. KHÔNG ghi đè config.macro_bias:
            // giữ nguyên BULLISH/BEARISH/NEUTRAL do user chọn, và giữ AUTO để EMA50/200
            // tiếp tục được đánh giá động ở mỗi tín hiệu.
            info!(
                "🤖 [Daily Scheduler] Nguồn báo cáo: {} | bias tư vấn: {} (config.macro_bias giữ nguyên).",
                scheduler.report_source(),
                report.macro_bias.as_str()
            );
            {
                let mut rep = state.daily_report.write().await;
                *rep = Some(report);
            }
            if let Err(err) = persist_state(&state).await {
                warn!("⚠️ Không lưu được Daily Macro report: {}", err);
            }
            info!("✅ [Daily Scheduler 07:00 AM] Đã cập nhật lịch tin & xu hướng EMA.");
        }
    });
}

/// Session Pre-Alert Worker — briefing thông tin trước phiên Âu/Mỹ, mỗi 30 giây.
pub fn spawn_session_alerts(state: AppState) {
    tokio::spawn(async move {
        info!("🌍 [Session Alerts] Worker briefing trước phiên đã khởi chạy (chu kỳ 30 giây)...");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            interval.tick().await;
            let sent = evaluate_and_dispatch(&state).await;
            if sent > 0 {
                info!("✅ [Session Alerts] Đã gửi {} briefing trước phiên.", sent);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Candle, Timeframe};

    fn snapshot(live_price: Option<f64>, last_close: f64) -> MarketFeedSnapshot {
        MarketFeedSnapshot {
            candles: vec![Candle {
                timestamp: 1,
                open: last_close,
                high: last_close,
                low: last_close,
                close: last_close,
                volume: 100.0,
                taker_buy_volume: 0.0,
                timeframe: Timeframe::M15,
            }],
            source: "BINANCE_PAXG".to_string(),
            live_price,
        }
    }

    #[test]
    fn spot_basis_uses_live_price_and_removes_manual_offset() {
        let snapshot = snapshot(Some(4106.10), 4100.0);
        let basis = compute_spot_basis(&snapshot, 4111.17, 2.0).expect("finite basis");
        assert!((basis - 7.07).abs() < 1e-9);
    }

    #[test]
    fn spot_basis_falls_back_to_last_close_and_clamps() {
        // spot 9000, PAXG 8850 -> basis 150 (2% = 180) hợp lệ, clamp về +100.
        let snapshot = snapshot(None, 8850.0);
        let basis = compute_spot_basis(&snapshot, 9000.0, 0.0).expect("finite basis");
        assert_eq!(basis, 100.0);

        assert_eq!(compute_spot_basis(&snapshot, 0.0, 0.0), None);
        assert_eq!(compute_spot_basis(&snapshot, f64::NAN, 0.0), None);
    }

    #[test]
    fn spot_basis_accepts_plausible_difference() {
        let snapshot = snapshot(Some(4095.0), 4095.0);
        let basis = compute_spot_basis(&snapshot, 4100.0, 0.0).expect("plausible basis");
        assert!((basis - 5.0).abs() < 1e-9);
    }

    #[test]
    fn spot_basis_rejects_implausible_difference() {
        let snapshot = snapshot(Some(4000.0), 4000.0);
        // 500 > max(4500*2%=90, 50) -> outlier.
        assert_eq!(compute_spot_basis(&snapshot, 4500.0, 0.0), None);
        // 100 > max(4100*2%=82, 50) -> outlier.
        assert_eq!(compute_spot_basis(&snapshot, 4100.0, 0.0), None);
    }

    #[test]
    fn spot_basis_rejects_bad_spot_price() {
        let snapshot = snapshot(Some(4000.0), 4000.0);
        for bad in [
            0.0,
            -1.0,
            499.99,
            10_000.01,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            assert_eq!(
                compute_spot_basis(&snapshot, bad, 0.0),
                None,
                "spot {bad} must be rejected"
            );
        }
    }

    #[test]
    fn htf_shift_uses_spot_basis_else_broker_offset() {
        assert!((htf_shift_amount(Some(5.07), 2.0) - 5.07).abs() < 1e-9);
        assert!((htf_shift_amount(None, 2.0) - 2.0).abs() < 1e-9);
        assert_eq!(htf_shift_amount(None, 0.0), 0.0);
        assert!(htf_shift_amount(None, f64::NAN).is_nan());
        assert!(htf_shift_amount(Some(f64::INFINITY), 2.0).is_infinite());
    }

    #[test]
    fn legacy_twelvedata_ohlcv_config_migrates_to_binance() {
        let mut config = BotConfig {
            data_source: "TWELVEDATA".to_string(),
            ..BotConfig::default()
        };
        migrate_ohlcv_source_to_binance(&mut config);
        assert_eq!(config.data_source, "BINANCE_PAXG");

        let mut already_binance = BotConfig::default();
        migrate_ohlcv_source_to_binance(&mut already_binance);
        assert_eq!(already_binance.data_source, "BINANCE_PAXG");
    }
}
