use axum::http::HeaderValue;
use gold_signal_bot::api::{create_router, AppState};
use gold_signal_bot::macro_flow;
use gold_signal_bot::market_feed::MarketDataFeed;
use gold_signal_bot::news_filter::{NewsFilterConfig, NewsFilterService};
use gold_signal_bot::scheduler::DailyScheduler;
use gold_signal_bot::storage::PersistentStore;
use gold_signal_bot::telegram::TelegramNotifier;
use gold_signal_bot::worker;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
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
    worker::normalize_data_source(&mut config);

    if config.telegram_bot_token.trim().is_empty() {
        warn!("🔐 Telegram token chưa cấu hình — dashboard sẽ không gửi được alert.");
    }
    let webhook_passphrase = config.webhook_passphrase.trim();
    if webhook_passphrase.is_empty() {
        warn!(
            "🔐 Webhook passphrase đang trống — chỉ an toàn trong Tailscale; nên đặt WEBHOOK_PASSPHRASE và cấu hình TradingView."
        );
    } else if webhook_passphrase.chars().count() < 16 {
        warn!(
            "🔐 Webhook passphrase chỉ dài {} ký tự — nên dùng tối thiểu 16 ký tự ngẫu nhiên.",
            webhook_passphrase.chars().count()
        );
    }

    // 3. OHLCV theo `data_source`: Binance PAXGUSDT (mặc định, có volume) hoặc
    // TwelveData XAU/USD + volume proxy PAXG. Spot /price chỉ dùng để căn basis PAXG.
    let market_feed = MarketDataFeed::new();
    let mut cached_spot: Option<(f64, Instant)> = None;

    let mut initial_snapshot = market_feed
        .fetch_market_snapshot(
            config.timeframe,
            300,
            &config.data_source,
            config.broker_price_offset,
        )
        .await;

    let initial_spot = worker::apply_spot_basis(
        &mut initial_snapshot,
        &config,
        &market_feed,
        &mut cached_spot,
    )
    .await;
    let (initial_spot_price, initial_spot_basis) = match initial_spot {
        Some((spot, basis)) => (Some(spot), Some(basis)),
        None => (None, None),
    };

    if initial_snapshot.source == "FALLBACK_SEED" {
        warn!(
            "⚠️ Không có dữ liệu thị trường thật lúc khởi động — dùng nến fallback cho UI, KHÔNG phân tích."
        );
    } else if initial_snapshot.source == "BINANCE_PAXG"
        && config.use_auto_spot_basis
        && initial_spot_basis.is_none()
    {
        warn!("⚠️ Không có spot XAU/USD lúc khởi động — chỉ dùng broker offset thủ công.");
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

    // 3b. Bối cảnh đa khung D1/H4/H1 (read-only) — cùng nguồn với nến tín hiệu.
    let initial_htf = worker::fetch_market_context(
        &market_feed,
        &config.data_source,
        initial_spot_basis,
        config.broker_price_offset,
    )
    .await;
    let (initial_market_context, initial_h4_candles, initial_d1_candles) = match initial_htf {
        Some((context, h4, d1)) => (Some(context), h4, d1),
        None => (None, Vec::new(), Vec::new()),
    };
    if let Some(context) = initial_market_context.as_ref() {
        info!(
            "🧭 [HTF Context] D1={} ({} nến) · H4={} ({} nến) · H1={} ({} nến) · source={}",
            context.d1.bias,
            context.d1.candles,
            context.h4.bias,
            context.h4.candles,
            context.h1.bias,
            context.h1.candles,
            context.price_source
        );
    } else {
        warn!("⚠️ [HTF Context] Chưa sẵn sàng — dashboard sẽ hiển thị trạng thái chờ.");
    }

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
        active_volume_source: Arc::new(RwLock::new("BINANCE_PAXG".to_string())),
        spot_price: Arc::new(RwLock::new(initial_spot_price)),
        spot_basis: Arc::new(RwLock::new(initial_spot_basis)),
        sent_session_alerts: Arc::new(RwLock::new(persisted.sent_session_alerts)),
        market_context: Arc::new(RwLock::new(initial_market_context)),
        htf_h4_candles: Arc::new(RwLock::new(initial_h4_candles)),
        htf_d1_candles: Arc::new(RwLock::new(initial_d1_candles)),
        signal_gate: Arc::new(tokio::sync::Mutex::new(())),
        persist_lock: Arc::new(tokio::sync::Mutex::new(())),
        last_scan_at: Arc::new(RwLock::new(None)),
        spot_basis_at: Arc::new(RwLock::new(initial_spot_basis.map(|_| chrono::Utc::now()))),
        divergence_alerts: Arc::new(RwLock::new(Vec::new())),
        divergence_last_sent: Arc::new(RwLock::new(std::collections::HashMap::new())),
        action_trigger_last_sent: Arc::new(RwLock::new(std::collections::HashMap::new())),
        macro_flow: Arc::new(RwLock::new(None)),
        macro_flow_updated_at: Arc::new(RwLock::new(None)),
        last_ai_decision: Arc::new(RwLock::new(None)),
    };

    // 6. Background Scanner 24/7 — quét mỗi 60 giây
    worker::spawn_scanner(state.clone(), cached_spot);

    // 6b. HTF Context Worker — refresh D1/H4/H1 mỗi 15 phút (read-only, không vào lệnh).
    worker::spawn_htf_context(state.clone());

    // 7. Daily Cron 07:00 AM — đồng bộ ForexFactory + báo cáo EMA
    worker::spawn_daily_scheduler(state.clone(), scheduler);

    // 8. Session Pre-Alert Worker — briefing thông tin trước phiên Âu/Mỹ, mỗi 30 giây.
    worker::spawn_session_alerts(state.clone());

    // 8b. Divergence Alerts Worker — cảnh báo phân kỳ RSI M15/H1/H4, mỗi 60 giây.
    // Độc lập hoàn toàn với tín hiệu/quota; không lưu vào `signals`.
    gold_signal_bot::divergence_alerts::spawn_divergence_worker(state.clone());

    // 8c. Action Trigger Worker — cảnh báo khi PRIMARY scenario chạm trigger trên
    // nến M15 đã đóng. Read-only: không quota, không chèn tín hiệu, không auto-trade.
    gold_signal_bot::action_alerts::spawn_action_trigger_worker(state.clone());

    // 8d. Macro & Flow Worker — FRED/Yahoo/CFTC read-only, refresh mỗi 15 phút.
    macro_flow::spawn_macro_flow_worker(state.clone());

    // 8e. Telegram command worker — /start, /stop, /view, /status, /help.
    // Chỉ admin chat được phép; không quota, không chèn tín hiệu.
    gold_signal_bot::telegram_commands::spawn_telegram_command_worker(state.clone());

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
