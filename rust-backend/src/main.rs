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
use tower_http::cors::{Any, CorsLayer};
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
    let ai_api_key = std::env::var("OPENROUTER_API_KEY")
        .or_else(|_| std::env::var("GEMINI_API_KEY"))
        .unwrap_or_default();

    // 2. Nạp trạng thái bền vững từ data/bot_state.json
    let store = PersistentStore::new("data/bot_state.json");
    let persisted = store.load();
    let mut config = persisted.config;
    if config.telegram_bot_token.trim().is_empty() {
        config.telegram_bot_token = env_bot_token.clone();
    }
    if config.telegram_chat_id.trim().is_empty() {
        config.telegram_chat_id = env_chat_id.clone();
    }

    // 3. Nguồn dữ liệu thật: Binance PAXGUSDT (mặc định) hoặc TwelveData XAU/USD
    let market_feed = MarketDataFeed::new();
    if market_feed.has_twelve_data_key() && config.data_source.eq_ignore_ascii_case("BINANCE_PAXG")
    {
        config.data_source = "TWELVEDATA".to_string();
    }

    let initial_candles = {
        let fetched = market_feed
            .fetch_closed_gold_candles(config.timeframe, 120)
            .await;
        if fetched.is_empty() {
            MarketDataFeed::generate_fallback_gold_candles(120, config.timeframe)
        } else {
            fetched
        }
    };
    info!(
        "📊 Đã nạp {} cây nến đã đóng cửa từ {}",
        initial_candles.len(),
        config.data_source
    );

    // 4. Nạp lịch tin kinh tế thật từ ForexFactory (fallback an toàn nếu offline)
    let scheduler = DailyScheduler::new(ai_api_key.clone(), None);
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
            let fresh_candles = scanner_state
                .market_feed
                .fetch_closed_gold_candles(config.timeframe, 120)
                .await;
            if !fresh_candles.is_empty() {
                *scanner_state.candles.write().await = fresh_candles;
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
            if let Some(mut signal) = GoldMarketAnalyzer::analyze_market(&candles, &config) {
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

            let scheduler = DailyScheduler::new(ai_api_key.clone(), None);
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

            {
                let mut cfg = cron_state.config.write().await;
                cfg.macro_bias = report.macro_bias.as_str().to_string();
            }
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

    // 8. CORS + Router
    let cors = CorsLayer::new()
        .allow_origin(Any)
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
