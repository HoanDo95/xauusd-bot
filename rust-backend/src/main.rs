use axum::http::HeaderValue;
use chrono::Utc;
use gold_signal_bot::analyzer::GoldMarketAnalyzer;
use gold_signal_bot::api::{create_router, persist_state, AppState};
use gold_signal_bot::market_feed::MarketDataFeed;
use gold_signal_bot::news_filter::{ImpactLevel, NewsFilterConfig, NewsFilterService};
use gold_signal_bot::scheduler::DailyScheduler;
use gold_signal_bot::storage::PersistentStore;
use gold_signal_bot::telegram::TelegramNotifier;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Tracing/Logging chuẩn production
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(true)
        .with_thread_ids(true)
        .compact()
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|err| format!("Thiết lập hệ thống log thất bại: {}", err))?;

    info!("🚀 Đang khởi động Gold Quant & Signal Bot (Rust High-Performance Architecture)...");

    dotenvy::dotenv().ok();

    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);
    let env_bot_token = std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
    let env_chat_id = std::env::var("TELEGRAM_CHAT_ID").unwrap_or_default();
    let env_webhook_passphrase = std::env::var("WEBHOOK_PASSPHRASE").unwrap_or_default();

    // 2. Nạp trạng thái bền vững từ data/bot_state.json
    let store = PersistentStore::new("data/bot_state.json");
    let persisted = store.load();
    let mut config = persisted.config.sanitized();
    if config.telegram_bot_token.trim().is_empty() {
        config.telegram_bot_token = env_bot_token.clone();
    }
    if config.telegram_chat_id.trim().is_empty() {
        config.telegram_chat_id = env_chat_id.clone();
    }
    if config.webhook_passphrase.trim().is_empty() {
        config.webhook_passphrase = env_webhook_passphrase;
    }

    // 3. Nguồn dữ liệu thật: Binance PAXGUSDT (mặc định) hoặc TwelveData XAU/USD
    let market_feed = MarketDataFeed::new();
    if market_feed.has_twelve_data_key() && config.data_source.eq_ignore_ascii_case("BINANCE_PAXG")
    {
        config.data_source = "TWELVEDATA".to_string();
    }

    let initial_snapshot = market_feed
        .fetch_market_snapshot(
            config.timeframe,
            300,
            &config.data_source,
            config.broker_price_offset,
        )
        .await;
    if initial_snapshot.source == "FALLBACK_SEED" {
        warn!(
            "⚠️ Không có dữ liệu thị trường thật lúc khởi động — dùng nến fallback cho UI, KHÔNG phân tích."
        );
    }
    let initial_live_price = initial_snapshot.live_price;
    let initial_data_source = initial_snapshot.source.clone();
    let initial_candles = initial_snapshot.candles;
    info!(
        "📊 Đã nạp {} cây nến đã đóng cửa từ {} (live={:?})",
        initial_candles.len(),
        initial_data_source,
        initial_live_price
    );

    // 4. Nạp lịch tin kinh tế thật từ ForexFactory (fallback an toàn nếu offline)
    // AI là tùy chọn; from_env tự chọn Gemini/OpenRouter hoặc EMA fallback.
    let scheduler = DailyScheduler::from_env();
    let mut news_filter = NewsFilterService::new(NewsFilterConfig::default());
    let initial_events = scheduler.fetch_daily_economic_events().await;
    info!("📅 Đã nạp {} sự kiện kinh tế USD.", initial_events.len());
    news_filter.update_events(initial_events);

    // 5. Khởi tạo Shared State
    let state = AppState {
        config: Arc::new(RwLock::new(config.clone())),
        candles: Arc::new(RwLock::new(initial_candles)),
        signals: Arc::new(RwLock::new(persisted.signals)),
        telegram: Arc::new(RwLock::new(TelegramNotifier::new(
            config.telegram_bot_token.clone(),
            config.telegram_chat_id.clone(),
        ))),
        news_filter: Arc::new(RwLock::new(news_filter)),
        daily_report: Arc::new(RwLock::new(persisted.daily_report)),
        store: Arc::new(store),
        market_feed: Arc::new(market_feed),
        live_price: Arc::new(RwLock::new(initial_live_price)),
        active_data_source: Arc::new(RwLock::new(initial_data_source)),
        sent_session_alerts: Arc::new(RwLock::new(persisted.sent_session_alerts)),
    };

    // 6. Background Scanner 24/7 — quét mỗi 60 giây
    let scanner_state = state.clone();
    tokio::spawn(async move {
        info!("⏱️ Sniper Worker 24/7 đã khởi chạy (chu kỳ 60 giây)...");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

        loop {
            interval.tick().await;

            // 6.1 Luôn refresh nến thật đã đóng cửa (kể cả khi bot tạm dừng).
            let config = scanner_state.config.read().await.clone();
            let snapshot = scanner_state
                .market_feed
                .fetch_market_snapshot(
                    config.timeframe,
                    300,
                    &config.data_source,
                    config.broker_price_offset,
                )
                .await;

            // Không bao giờ phân tích/tracker trên nến giả fallback.
            if snapshot.source == "FALLBACK_SEED" {
                warn!(
                    "⚠️ [Scanner] Cả Binance và TwelveData đều lỗi — giữ nguyên nến cũ, bỏ qua tracker & phân tích."
                );
                continue;
            }

            let fresh_candles = snapshot.candles;
            if !fresh_candles.is_empty() {
                // Cập nhật TP1/TP2/SL cho các tín hiệu đang mở trước khi ghi đè nến.
                let updates = {
                    let mut signals = scanner_state.signals.write().await;
                    gold_signal_bot::tracker::evaluate_active_signals(&mut signals, &fresh_candles)
                };
                *scanner_state.candles.write().await = fresh_candles;
                *scanner_state.live_price.write().await = snapshot.live_price;
                *scanner_state.active_data_source.write().await = snapshot.source.clone();
                if !updates.is_empty() {
                    info!("📈 Đã cập nhật {} tín hiệu theo TP/SL", updates.len());
                    for update in updates.iter().filter(|signal| !signal.is_simulated) {
                        let telegram = scanner_state.telegram.read().await.clone();
                        if let Err(err) = telegram.send_outcome_alert(update).await {
                            warn!("⚠️ Không gửi được Telegram outcome: {}", err);
                        }
                    }
                    if let Err(err) = persist_state(&scanner_state).await {
                        warn!("⚠️ Không lưu được kết quả TP/SL: {}", err);
                    }
                }
            }

            // 6.2 Bot đang hoạt động?
            if !config.is_active {
                continue;
            }

            let now = Utc::now();
            let signals_snapshot = scanner_state.signals.read().await.clone();

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
            let is_market_safe = scanner_state
                .news_filter
                .read()
                .await
                .check_kill_switch(now, &config);
            if !is_market_safe {
                continue;
            }

            // 6.6 Dedup: cùng một cây nến chỉ được đánh giá/phát tín hiệu tối đa 1 lần.
            let candles = scanner_state.candles.read().await.clone();
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
            let live_price = *scanner_state.live_price.read().await;
            if let Some(mut signal) = GoldMarketAnalyzer::analyze_market_with_live_price(
                &candles,
                &config,
                live_price,
                Utc::now().timestamp_millis(),
            ) {
                if PersistentStore::is_duplicate_candle(&signals_snapshot, &signal.dedup_key) {
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

                let telegram = scanner_state.telegram.read().await.clone();
                if let Ok(msg_id) = telegram.send_signal_alert(&signal).await {
                    signal.telegram_message_id = Some(msg_id);
                }

                signal.is_simulated = false;
                scanner_state.signals.write().await.insert(0, signal);

                if let Err(err) = persist_state(&scanner_state).await {
                    warn!("⚠️ Không lưu được trạng thái sau tín hiệu: {}", err);
                }
            }
        }
    });

    // 7. Daily Cron 07:00 AM — đồng bộ ForexFactory + báo cáo EMA
    let cron_state = state.clone();
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
                let mut nf = cron_state.news_filter.write().await;
                nf.update_events(events.clone());
            }

            let candles = cron_state.candles.read().await.clone();
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
                let mut rep = cron_state.daily_report.write().await;
                *rep = Some(report);
            }
            if let Err(err) = persist_state(&cron_state).await {
                warn!("⚠️ Không lưu được Daily Macro report: {}", err);
            }
            info!("✅ [Daily Scheduler 07:00 AM] Đã cập nhật lịch tin & xu hướng EMA.");
        }
    });

    // 8. Session Pre-Alert Worker — briefing thông tin trước phiên Âu/Mỹ, mỗi 30 giây.
    let session_state = state.clone();
    tokio::spawn(async move {
        info!("🌍 [Session Alerts] Worker briefing trước phiên đã khởi chạy (chu kỳ 30 giây)...");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            interval.tick().await;
            let sent = gold_signal_bot::session_alerts::evaluate_and_dispatch(&session_state).await;
            if sent > 0 {
                info!("✅ [Session Alerts] Đã gửi {} briefing trước phiên.", sent);
            }
        }
    });

    // 9. CORS + Router
    // Same-origin dashboard không cần CORS; chỉ mở cho các origin được cấu hình.
    let cors_origins = std::env::var("CORS_ORIGINS").unwrap_or_else(|_| {
        "http://localhost:3000,http://127.0.0.1:3000,https://xauusd-bot.tailec308e.ts.net"
            .to_string()
    });
    let origins: Vec<HeaderValue> = cors_origins
        .split(',')
        .filter_map(|origin| origin.trim().parse().ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods(Any)
        .allow_headers(Any);

    let app = create_router(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(
        "🌐 Axum REST API & Webhook Server đang lắng nghe tại: http://{}",
        addr
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
