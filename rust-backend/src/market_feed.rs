use crate::exness_feed::{
    exness_candles, load_exness_file, timeframe_label, DEFAULT_EXNESS_PATH, EXNESS_MAX_AGE_SECS,
    SOURCE_EXNESS, VOLUME_SOURCE_EXNESS_TICK,
};
use crate::models::{Candle, Timeframe};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Nhãn nguồn volume ổn định cho dashboard/health.
pub const VOLUME_SOURCE_BINANCE_PAXG: &str = "BINANCE_PAXG";
pub const VOLUME_SOURCE_BINANCE_PAXG_PROXY: &str = "BINANCE_PAXG_PROXY";
pub const VOLUME_SOURCE_OANDA_TICK: &str = "OANDA_TICK";
pub const VOLUME_SOURCE_YAHOO_FUTURES_PROXY: &str = "YAHOO_FUTURES_PROXY";
pub const VOLUME_SOURCE_NONE: &str = "NONE";
pub const SOURCE_OANDA_XAUUSD: &str = "OANDA_XAUUSD";
pub const SOURCE_TWELVEDATA_YAHOO_VOLUME: &str = "TWELVEDATA_XAUUSD+YAHOO_FUTURES_VOLUME";

/// Yahoo chart endpoint for COMEX gold futures. `period1/period2` is required:
/// the `range=` variant returns HTTP 429 from the production network.
const YAHOO_GC_CHART_URL: &str = "https://query1.finance.yahoo.com/v8/finance/chart/GC=F";
const YAHOO_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
const YAHOO_MAX_RETRIES: usize = 2;
const YAHOO_FAILURE_THRESHOLD: u32 = 3;
const YAHOO_COOLDOWN_SECS: u64 = 30 * 60;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Trạng thái circuit-breaker cho Yahoo để tránh spam khi bị 429.
#[derive(Debug, Default)]
struct YahooFailureState {
    consecutive: u32,
    cooldown_until: Option<Instant>,
}

/// Ảnh chụp dữ liệu thị trường tại một thời điểm: nến đã đóng + giá live + nguồn.
#[derive(Debug, Clone)]
pub struct MarketFeedSnapshot {
    /// Chỉ chứa các cây nến đã đóng cửa hoàn tất.
    pub candles: Vec<Candle>,
    /// "TWELVEDATA" | "BINANCE_PAXG" | "FALLBACK_SEED".
    pub source: String,
    /// Giá live từ cây nến đang chạy (nếu có).
    pub live_price: Option<f64>,
    /// "BINANCE_PAXG" | "BINANCE_PAXG_PROXY" | "NONE".
    pub volume_source: String,
}

impl MarketFeedSnapshot {
    fn fallback(limit: usize, timeframe: Timeframe) -> Self {
        Self {
            candles: MarketDataFeed::generate_fallback_gold_candles(limit, timeframe),
            source: "FALLBACK_SEED".to_string(),
            live_price: None,
            volume_source: VOLUME_SOURCE_NONE.to_string(),
        }
    }
}

/// Cache of Yahoo volume-only candles keyed by timeframe label.
type YahooVolumeCache = Arc<tokio::sync::Mutex<HashMap<String, (Instant, Vec<Candle>)>>>;

/// Nguồn dữ liệu OHLCV Vàng miễn phí 24/7.
/// - `data_source=TWELVEDATA`: OHLC từ TwelveData `XAU/USD` + volume Yahoo GC=F
///   (fallback Binance PAXG, rồi NONE).
/// - Mặc định dùng Binance Public API `PAXGUSDT` (không cần API key).
/// - Offline: fallback về bộ sinh nến mẫu an toàn.
#[derive(Debug, Clone)]
pub struct MarketDataFeed {
    client: Client,
    twelve_data_api_key: Option<String>,
    twelve_cache: Arc<tokio::sync::Mutex<HashMap<String, (Instant, MarketFeedSnapshot)>>>,
    oanda_api_token: Option<String>,
    oanda_base_url: String,
    oanda_cache: Arc<tokio::sync::Mutex<HashMap<String, (Instant, MarketFeedSnapshot)>>>,
    /// Hard kill from `YAHOO_FUTURES_VOLUME_ENABLED=false` at startup.
    yahoo_futures_volume_env_enabled: bool,
    /// Runtime toggle from `BotConfig.yahoo_futures_volume_enabled`.
    yahoo_futures_volume_config_enabled: Arc<AtomicBool>,
    yahoo_cache: YahooVolumeCache,
    yahoo_failures: Arc<tokio::sync::Mutex<YahooFailureState>>,
    /// Read-only Exness MT5 JSON feed (`data/exness_bars.json`).
    exness_path: PathBuf,
}

impl MarketDataFeed {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap_or_else(|_| Client::new());

        let twelve_data_api_key = std::env::var("TWELVEDATA_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let oanda_api_token = std::env::var("OANDA_API_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let oanda_base_url = match std::env::var("OANDA_ENV")
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str()
        {
            "live" | "trade" => "https://api-fxtrade.oanda.com",
            _ => "https://api-fxpractice.oanda.com",
        }
        .to_string();
        let yahoo_futures_volume_env_enabled = std::env::var("YAHOO_FUTURES_VOLUME_ENABLED")
            .ok()
            .map(|value| {
                !matches!(
                    value.trim().to_lowercase().as_str(),
                    "false" | "0" | "no" | "off"
                )
            })
            .unwrap_or(true);

        Self {
            client,
            twelve_data_api_key,
            twelve_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            oanda_api_token,
            oanda_base_url,
            oanda_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            yahoo_futures_volume_env_enabled,
            yahoo_futures_volume_config_enabled: Arc::new(AtomicBool::new(true)),
            yahoo_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            yahoo_failures: Arc::new(tokio::sync::Mutex::new(YahooFailureState::default())),
            exness_path: PathBuf::from(
                std::env::var("EXNESS_BARS_PATH")
                    .unwrap_or_else(|_| DEFAULT_EXNESS_PATH.to_string()),
            ),
        }
    }

    /// Runtime toggle from `BotConfig`. The env var remains a hard kill.
    pub fn set_yahoo_futures_volume_enabled(&self, enabled: bool) {
        self.yahoo_futures_volume_config_enabled
            .store(enabled, Ordering::Relaxed);
    }

    pub fn yahoo_futures_volume_enabled(&self) -> bool {
        self.yahoo_futures_volume_env_enabled
            && self
                .yahoo_futures_volume_config_enabled
                .load(Ordering::Relaxed)
    }

    /// Test/runtime override for the env-level kill switch.
    pub fn with_yahoo_futures_volume_enabled(mut self, enabled: bool) -> Self {
        self.yahoo_futures_volume_env_enabled = enabled;
        self
    }

    /// Cho phép test hoặc cấu hình runtime override API key.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        let value = api_key.into().trim().to_string();
        self.twelve_data_api_key = if value.is_empty() { None } else { Some(value) };
        self
    }

    pub fn has_twelve_data_key(&self) -> bool {
        self.twelve_data_api_key.is_some()
    }

    pub fn has_oanda_token(&self) -> bool {
        self.oanda_api_token.is_some()
    }

    /// Test/runtime override for the OANDA token.
    pub fn with_oanda_token(mut self, token: impl Into<String>) -> Self {
        let value = token.into().trim().to_string();
        self.oanda_api_token = if value.is_empty() { None } else { Some(value) };
        self
    }

    async fn cached_oanda_snapshot(&self, label: &str) -> Option<MarketFeedSnapshot> {
        let cache = self.oanda_cache.lock().await;
        cache.get(label).and_then(|(fetched_at, snapshot)| {
            if fetched_at.elapsed() < twelve_cache_ttl(label) && !snapshot.candles.is_empty() {
                Some(snapshot.clone())
            } else {
                None
            }
        })
    }

    async fn store_oanda_snapshot(&self, label: &str, snapshot: &MarketFeedSnapshot) {
        if snapshot.candles.is_empty() {
            return;
        }
        let mut cache = self.oanda_cache.lock().await;
        cache.insert(label.to_string(), (Instant::now(), snapshot.clone()));
    }

    async fn cached_twelve_snapshot(&self, label: &str) -> Option<MarketFeedSnapshot> {
        let cache = self.twelve_cache.lock().await;
        cache.get(label).and_then(|(fetched_at, snapshot)| {
            if fetched_at.elapsed() < twelve_cache_ttl(label) && !snapshot.candles.is_empty() {
                Some(snapshot.clone())
            } else {
                None
            }
        })
    }

    async fn store_twelve_snapshot(&self, label: &str, snapshot: &MarketFeedSnapshot) {
        if snapshot.candles.is_empty() {
            return;
        }
        let mut cache = self.twelve_cache.lock().await;
        cache.insert(label.to_string(), (Instant::now(), snapshot.clone()));
    }

    /// Lấy giá spot XAU/USD tham chiếu (không volume) để tự động căn basis PAXG.
    /// - Ưu tiên TwelveData `/price` nếu có `TWELVEDATA_API_KEY`.
    /// - Fallback miễn phí `https://api.gold-api.com/price/XAU`.
    pub async fn fetch_spot_xau_usd(&self) -> Option<f64> {
        if let Some(api_key) = self.twelve_data_api_key.as_deref() {
            let url = format!(
                "https://api.twelvedata.com/price?symbol=XAU/USD&apikey={}",
                api_key
            );
            match self.fetch_spot_price_url(&url, "TwelveData XAU/USD").await {
                Some(price) => {
                    info!("🌐 [MarketFeed] Spot XAU/USD (TwelveData): {:.2}", price);
                    return Some(price);
                }
                None => {
                    warn!("⚠️ TwelveData spot XAU/USD không khả dụng, thử gold-api.com.");
                }
            }
        }

        match self
            .fetch_spot_price_url("https://api.gold-api.com/price/XAU", "gold-api.com XAU")
            .await
        {
            Some(price) => {
                info!("🌐 [MarketFeed] Spot XAU/USD (gold-api.com): {:.2}", price);
                Some(price)
            }
            None => {
                warn!("⚠️ Không lấy được giá spot XAU/USD từ mọi nguồn.");
                None
            }
        }
    }

    async fn fetch_spot_price_url(&self, url: &str, provider: &str) -> Option<f64> {
        let response = match self.client.get(url).send().await {
            Ok(response) => response,
            Err(err) => {
                warn!("⚠️ {} spot request lỗi: {}", provider, err.without_url());
                return None;
            }
        };

        if !response.status().is_success() {
            warn!("⚠️ {} spot HTTP {}", provider, response.status());
            return None;
        }

        let payload: Value = match response.json().await {
            Ok(payload) => payload,
            Err(err) => {
                warn!("⚠️ {} spot JSON lỗi: {}", provider, err.without_url());
                return None;
            }
        };

        let price = Self::parse_spot_price_json(&payload);
        if price.is_none() {
            warn!(
                "⚠️ {} spot payload không có giá hợp lệ: {}",
                provider, payload
            );
        }
        price
    }

    /// Parse `{"price":"4111.17"}` hoặc `{"price":4111.17}`; chỉ nhận giá hữu hạn > 0.
    pub(crate) fn parse_spot_price_json(body: &serde_json::Value) -> Option<f64> {
        let price = value_to_f64(body.get("price")?)?;
        if price.is_finite() && price > 0.0 {
            Some(price)
        } else {
            None
        }
    }

    /// Kéo nến VÀ CHỈ trả về các cây nến đã đóng cửa hoàn tất (anti-repaint).
    /// Wrapper tương thích cho các caller cũ không cần live price.
    pub async fn fetch_closed_gold_candles(
        &self,
        timeframe: Timeframe,
        limit: usize,
    ) -> Vec<Candle> {
        let preferred = if self.oanda_api_token.is_some() {
            "OANDA"
        } else if self.twelve_data_api_key.is_some() {
            "TWELVEDATA"
        } else {
            "BINANCE_PAXG"
        };
        self.fetch_market_snapshot(timeframe, limit, preferred, 0.0)
            .await
            .candles
    }

    /// Nến HTF đã đóng cửa cho bối cảnh D1/H4/H1 — Binance PAXGUSDT.
    /// Wrapper tương thích cho caller cũ.
    pub async fn fetch_htf_candles(&self, timeframe: Timeframe, limit: usize) -> Vec<Candle> {
        self.fetch_htf_candles_with_source(timeframe, limit, "BINANCE_PAXG")
            .await
            .0
    }

    /// Nến HTF theo nguồn đã chọn. TwelveData trả OHLC; volume được ghép từ
    /// Binance PAXG nếu timestamp khớp. Trả `(candles, price_source)`.
    pub async fn fetch_htf_candles_with_source(
        &self,
        timeframe: Timeframe,
        limit: usize,
        source: &str,
    ) -> (Vec<Candle>, String) {
        if source.trim().eq_ignore_ascii_case("EXNESS") {
            if let Some(candles) =
                self.exness_htf_candles(timeframe_label(timeframe), timeframe, limit)
            {
                return (candles, SOURCE_EXNESS.to_string());
            }
            warn!(
                "⚠️ Exness HTF {} không khả dụng/cũ, fallback sang nguồn khác.",
                timeframe
            );
        }
        let use_oanda = source.trim().eq_ignore_ascii_case("OANDA");
        if use_oanda && self.oanda_api_token.is_some() {
            if let Some(snapshot) = self.fetch_oanda_snapshot(timeframe, limit).await {
                return (snapshot.candles, SOURCE_OANDA_XAUUSD.to_string());
            }
            warn!(
                "⚠️ OANDA HTF {} không khả dụng, fallback sang nguồn khác.",
                timeframe
            );
        }
        let use_twelve = source.trim().eq_ignore_ascii_case("TWELVEDATA")
            || (use_oanda && self.has_twelve_data_key());
        if use_twelve && self.has_twelve_data_key() {
            let yahoo_enabled = self.yahoo_futures_volume_enabled();
            let (twelve, binance, yahoo) = tokio::join!(
                self.fetch_twelve_snapshot(timeframe, limit),
                self.fetch_binance_closed_candles(binance_interval(timeframe), timeframe, limit),
                async {
                    if yahoo_enabled {
                        self.fetch_yahoo_volume(timeframe, limit).await
                    } else {
                        None
                    }
                }
            );
            let binance = binance.ok();
            if let Some(mut snapshot) = twelve {
                let price_source = Self::twelve_htf_price_source(
                    &mut snapshot.candles,
                    binance.as_deref(),
                    yahoo.as_deref(),
                );
                return (snapshot.candles, price_source);
            }
            warn!(
                "⚠️ TwelveData HTF {} không khả dụng, chuyển sang Binance PAXGUSDT.",
                timeframe
            );
            return (
                binance.unwrap_or_default(),
                VOLUME_SOURCE_BINANCE_PAXG.to_string(),
            );
        }

        (
            self.fetch_binance_closed_candles(binance_interval(timeframe), timeframe, limit)
                .await
                .unwrap_or_default(),
            VOLUME_SOURCE_BINANCE_PAXG.to_string(),
        )
    }

    /// Nến ngày (1d) từ nguồn đã chọn. `models::Timeframe` chưa có D1 nên nến
    /// được gắn tag H4 — chỉ dùng nội bộ để tính context, không phát tín hiệu.
    pub async fn fetch_daily_htf_candles(&self, limit: usize) -> Vec<Candle> {
        self.fetch_daily_htf_candles_with_source(limit, "BINANCE_PAXG")
            .await
            .0
    }

    pub async fn fetch_daily_htf_candles_with_source(
        &self,
        limit: usize,
        source: &str,
    ) -> (Vec<Candle>, String) {
        if source.trim().eq_ignore_ascii_case("EXNESS") {
            if let Some(candles) = self.exness_htf_candles("D1", Timeframe::H4, limit) {
                return (candles, SOURCE_EXNESS.to_string());
            }
            warn!("⚠️ Exness D1 không khả dụng/cũ, fallback sang nguồn khác.");
        }
        let use_oanda = source.trim().eq_ignore_ascii_case("OANDA");
        if use_oanda && self.oanda_api_token.is_some() {
            if let Some(snapshot) = self.fetch_oanda_daily_snapshot(limit).await {
                return (snapshot.candles, SOURCE_OANDA_XAUUSD.to_string());
            }
            warn!("⚠️ OANDA D1 không khả dụng, fallback sang nguồn khác.");
        }
        let use_twelve = source.trim().eq_ignore_ascii_case("TWELVEDATA")
            || (use_oanda && self.has_twelve_data_key());
        if use_twelve && self.has_twelve_data_key() {
            let yahoo_enabled = self.yahoo_futures_volume_enabled();
            let (twelve, binance, yahoo) = tokio::join!(
                self.fetch_twelve_daily_snapshot(limit),
                self.fetch_binance_closed_candles("1d", Timeframe::H4, limit),
                async {
                    if yahoo_enabled {
                        self.fetch_yahoo_daily_volume(limit).await
                    } else {
                        None
                    }
                }
            );
            let binance = binance.ok();
            if let Some(mut snapshot) = twelve {
                let price_source = Self::twelve_htf_price_source(
                    &mut snapshot.candles,
                    binance.as_deref(),
                    yahoo.as_deref(),
                );
                return (snapshot.candles, price_source);
            }
            warn!("⚠️ TwelveData D1 không khả dụng, chuyển sang Binance PAXGUSDT.");
            return (
                binance.unwrap_or_default(),
                VOLUME_SOURCE_BINANCE_PAXG.to_string(),
            );
        }

        (
            self.fetch_binance_closed_candles("1d", Timeframe::H4, limit)
                .await
                .unwrap_or_default(),
            VOLUME_SOURCE_BINANCE_PAXG.to_string(),
        )
    }

    fn twelve_htf_price_source(
        candles: &mut [Candle],
        binance_candles: Option<&[Candle]>,
        yahoo_candles: Option<&[Candle]>,
    ) -> String {
        if let Some(yahoo) = yahoo_candles {
            let matches = Self::count_volume_matches(candles, yahoo);
            if !candles.is_empty() && matches * 2 >= candles.len() {
                Self::merge_volume_proxy(candles, yahoo);
                return SOURCE_TWELVEDATA_YAHOO_VOLUME.to_string();
            }
        }

        let matched = binance_candles
            .map(|volume_candles| Self::merge_volume_proxy(candles, volume_candles))
            .unwrap_or(false);
        if matched {
            "TWELVEDATA_XAUUSD+PAXG_VOLUME".to_string()
        } else {
            "TWELVEDATA_XAUUSD".to_string()
        }
    }

    /// Đếm số nến khớp timestamp giữa giá và nguồn volume.
    fn count_volume_matches(price_candles: &[Candle], volume_candles: &[Candle]) -> usize {
        if price_candles.is_empty() || volume_candles.is_empty() {
            return 0;
        }
        let timestamps: std::collections::HashSet<i64> = volume_candles
            .iter()
            .map(|candle| candle.timestamp)
            .collect();
        price_candles
            .iter()
            .filter(|candle| timestamps.contains(&candle.timestamp))
            .count()
    }

    /// Match count over the most recent `window` price candles. M15 history can
    /// be dominated by weekend flat bars that Yahoo GC=F does not have, so the
    /// overall ratio is not a good acceptance test for the live volume filter.
    fn recent_volume_matches(
        price_candles: &[Candle],
        volume_candles: &[Candle],
        window: usize,
    ) -> (usize, usize) {
        let start = price_candles.len().saturating_sub(window);
        let recent = &price_candles[start..];
        (
            Self::count_volume_matches(recent, volume_candles),
            recent.len(),
        )
    }

    /// Ghép volume/taker-buy từ Binance PAXG vào nến TwelveData theo timestamp.
    /// Trả `true` nếu có ít nhất một cây nến khớp.
    fn merge_volume_proxy(price_candles: &mut [Candle], volume_candles: &[Candle]) -> bool {
        if price_candles.is_empty() || volume_candles.is_empty() {
            return false;
        }

        let volume_by_timestamp: HashMap<i64, (f64, f64)> = volume_candles
            .iter()
            .map(|candle| (candle.timestamp, (candle.volume, candle.taker_buy_volume)))
            .collect();

        let mut matched = 0usize;
        for candle in price_candles {
            if let Some((volume, taker_buy_volume)) = volume_by_timestamp.get(&candle.timestamp) {
                candle.volume = *volume;
                candle.taker_buy_volume = *taker_buy_volume;
                matched += 1;
            }
        }

        matched > 0
    }

    /// Volume-only candles from Yahoo GC=F (COMEX futures). Returns `None` when
    /// the feed is disabled, the circuit breaker is open, or the request fails.
    async fn fetch_yahoo_volume(&self, timeframe: Timeframe, limit: usize) -> Option<Vec<Candle>> {
        if !self.yahoo_futures_volume_enabled() {
            return None;
        }
        let label = twelve_cache_label(timeframe);
        if let Some(cached) = self.cached_yahoo_volume(label).await {
            return Some(cached);
        }

        let interval = yahoo_interval(timeframe);
        let lookback_days = yahoo_lookback_days(timeframe, limit);
        let payload = self.fetch_yahoo_chart(interval, lookback_days).await?;
        let now_ms = Utc::now().timestamp_millis();
        let candles = match timeframe {
            Timeframe::H4 => {
                let h1 = Self::parse_yahoo_chart(
                    &payload,
                    Timeframe::H1,
                    timeframe_millis(Timeframe::H1),
                    now_ms,
                );
                Self::aggregate_candles(&h1, Timeframe::H4, now_ms)
            }
            _ => Self::parse_yahoo_chart(&payload, timeframe, timeframe_millis(timeframe), now_ms),
        };

        if candles.is_empty() {
            warn!("⚠️ Yahoo GC=F {} không có nến volume hợp lệ.", label);
            return None;
        }
        self.store_yahoo_volume(label, &candles).await;
        info!(
            "📡 [MarketFeed] Yahoo GC=F {} volume: {} nến đã đóng.",
            label,
            candles.len()
        );
        Some(candles)
    }

    async fn fetch_yahoo_daily_volume(&self, limit: usize) -> Option<Vec<Candle>> {
        if !self.yahoo_futures_volume_enabled() {
            return None;
        }
        let label = "D1";
        if let Some(cached) = self.cached_yahoo_volume(label).await {
            return Some(cached);
        }

        let lookback_days = (limit.max(5) as i64) + 60;
        let payload = self.fetch_yahoo_chart("1d", lookback_days).await?;
        let now_ms = Utc::now().timestamp_millis();
        let mut candles = Self::parse_yahoo_chart(&payload, Timeframe::H4, DAY_MS, now_ms);
        for candle in &mut candles {
            candle.timestamp = yahoo_daily_timestamp(candle.timestamp);
            candle.timeframe = Timeframe::H4;
        }
        candles.sort_by_key(|candle| candle.timestamp);
        candles.dedup_by_key(|candle| candle.timestamp);

        if candles.is_empty() {
            warn!("⚠️ Yahoo GC=F D1 không có nến volume hợp lệ.");
            return None;
        }
        self.store_yahoo_volume(label, &candles).await;
        info!(
            "📡 [MarketFeed] Yahoo GC=F D1 volume: {} nến đã đóng.",
            candles.len()
        );
        Some(candles)
    }

    async fn fetch_yahoo_chart(&self, interval: &str, lookback_days: i64) -> Option<Value> {
        if self.yahoo_in_cooldown().await {
            return None;
        }

        let now = Utc::now().timestamp();
        let period1 = now.saturating_sub(lookback_days.max(1) * 24 * 60 * 60);
        let url = format!(
            "{}?interval={}&period1={}&period2={}",
            YAHOO_GC_CHART_URL, interval, period1, now
        );

        for attempt in 0..=YAHOO_MAX_RETRIES {
            let response = self
                .client
                .get(&url)
                .header("User-Agent", YAHOO_USER_AGENT)
                .header("Accept", "application/json")
                .send()
                .await;

            match response {
                Ok(response) if response.status().is_success() => {
                    match response.json::<Value>().await {
                        Ok(payload) => {
                            self.record_yahoo_success().await;
                            return Some(payload);
                        }
                        Err(err) => {
                            warn!(
                                "⚠️ Yahoo GC=F JSON lỗi (attempt {}): {}",
                                attempt + 1,
                                err.without_url()
                            );
                        }
                    }
                }
                Ok(response) => {
                    let status = response.status();
                    warn!(
                        "⚠️ Yahoo GC=F HTTP {} (attempt {}/{}).",
                        status,
                        attempt + 1,
                        YAHOO_MAX_RETRIES + 1
                    );
                    if status.as_u16() != 429 && !status.is_server_error() {
                        break;
                    }
                }
                Err(err) => {
                    warn!(
                        "⚠️ Yahoo GC=F request lỗi (attempt {}): {}",
                        attempt + 1,
                        err.without_url()
                    );
                }
            }

            if attempt < YAHOO_MAX_RETRIES {
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            }
        }

        self.record_yahoo_failure().await;
        None
    }

    async fn yahoo_in_cooldown(&self) -> bool {
        let state = self.yahoo_failures.lock().await;
        state
            .cooldown_until
            .is_some_and(|until| until > Instant::now())
    }

    async fn record_yahoo_success(&self) {
        let mut state = self.yahoo_failures.lock().await;
        state.consecutive = 0;
        state.cooldown_until = None;
    }

    async fn record_yahoo_failure(&self) {
        let mut state = self.yahoo_failures.lock().await;
        state.consecutive = state.consecutive.saturating_add(1);
        if state.consecutive >= YAHOO_FAILURE_THRESHOLD {
            state.cooldown_until = Some(Instant::now() + Duration::from_secs(YAHOO_COOLDOWN_SECS));
            warn!(
                "⚠️ Yahoo GC=F tạm nghỉ {} phút sau {} lần lỗi liên tiếp.",
                YAHOO_COOLDOWN_SECS / 60,
                state.consecutive
            );
        }
    }

    async fn cached_yahoo_volume(&self, label: &str) -> Option<Vec<Candle>> {
        let cache = self.yahoo_cache.lock().await;
        cache.get(label).and_then(|(fetched_at, candles)| {
            if fetched_at.elapsed() < twelve_cache_ttl(label) && !candles.is_empty() {
                Some(candles.clone())
            } else {
                None
            }
        })
    }

    async fn store_yahoo_volume(&self, label: &str, candles: &[Candle]) {
        if candles.is_empty() {
            return;
        }
        let mut cache = self.yahoo_cache.lock().await;
        cache.insert(label.to_string(), (Instant::now(), candles.to_vec()));
    }

    fn parse_yahoo_chart(
        payload: &Value,
        timeframe: Timeframe,
        step_ms: i64,
        now_ms: i64,
    ) -> Vec<Candle> {
        let Some(result) = payload
            .get("chart")
            .and_then(|chart| chart.get("result"))
            .and_then(|result| result.as_array())
            .and_then(|result| result.first())
        else {
            return Vec::new();
        };
        let Some(timestamps) = result.get("timestamp").and_then(|value| value.as_array()) else {
            return Vec::new();
        };
        let Some(quote) = result
            .get("indicators")
            .and_then(|indicators| indicators.get("quote"))
            .and_then(|quote| quote.as_array())
            .and_then(|quote| quote.first())
        else {
            return Vec::new();
        };
        let opens = quote.get("open").and_then(|value| value.as_array());
        let highs = quote.get("high").and_then(|value| value.as_array());
        let lows = quote.get("low").and_then(|value| value.as_array());
        let closes = quote.get("close").and_then(|value| value.as_array());
        let volumes = quote.get("volume").and_then(|value| value.as_array());

        let mut candles = Vec::new();
        for (index, timestamp) in timestamps.iter().enumerate() {
            let Some(timestamp_secs) = value_to_i64(timestamp) else {
                continue;
            };
            let timestamp_ms = timestamp_secs.saturating_mul(1000);
            if timestamp_ms.saturating_add(step_ms) > now_ms {
                continue;
            }
            let (Some(open), Some(high), Some(low), Some(close), Some(volume)) = (
                opens
                    .and_then(|values| values.get(index))
                    .and_then(value_to_f64),
                highs
                    .and_then(|values| values.get(index))
                    .and_then(value_to_f64),
                lows.and_then(|values| values.get(index))
                    .and_then(value_to_f64),
                closes
                    .and_then(|values| values.get(index))
                    .and_then(value_to_f64),
                volumes
                    .and_then(|values| values.get(index))
                    .and_then(value_to_f64),
            ) else {
                continue;
            };
            if !is_finite_ohlcv(open, high, low, close, volume) {
                continue;
            }
            candles.push(Candle {
                timestamp: timestamp_ms,
                open,
                high,
                low,
                close,
                volume,
                taker_buy_volume: 0.0,
                timeframe,
            });
        }

        candles.sort_by_key(|candle| candle.timestamp);
        candles.dedup_by_key(|candle| candle.timestamp);
        candles
    }

    fn aggregate_candles(candles: &[Candle], target: Timeframe, now_ms: i64) -> Vec<Candle> {
        let step_ms = timeframe_millis(target);
        let mut buckets: BTreeMap<i64, Candle> = BTreeMap::new();

        for candle in candles {
            let bucket = candle.timestamp - candle.timestamp.rem_euclid(step_ms);
            if bucket.saturating_add(step_ms) > now_ms {
                continue;
            }
            buckets
                .entry(bucket)
                .and_modify(|aggregate| {
                    aggregate.high = aggregate.high.max(candle.high);
                    aggregate.low = aggregate.low.min(candle.low);
                    aggregate.close = candle.close;
                    aggregate.volume += candle.volume;
                })
                .or_insert(Candle {
                    timestamp: bucket,
                    open: candle.open,
                    high: candle.high,
                    low: candle.low,
                    close: candle.close,
                    volume: candle.volume,
                    taker_buy_volume: 0.0,
                    timeframe: target,
                });
        }

        buckets.into_values().collect()
    }

    async fn fetch_binance_closed_candles(
        &self,
        interval: &str,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<Vec<Candle>, String> {
        let limit = limit.max(5);
        // limit + 1 để luôn có cây nến vừa đóng cửa ngay cả khi API trả cây đang chạy.
        let url = format!(
            "https://api.binance.com/api/v3/klines?symbol=PAXGUSDT&interval={}&limit={}",
            interval,
            limit + 1
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Binance HTF request error: {}", e.without_url()))?;

        if !response.status().is_success() {
            return Err(format!("Binance HTF HTTP {}", response.status()));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("Binance HTF JSON parse error: {}", e.without_url()))?;

        let (candles, _) = Self::parse_binance_klines_snapshot_with_now(
            &payload,
            timeframe,
            Utc::now().timestamp_millis(),
        );

        info!(
            "📡 [MarketFeed] Binance PAXGUSDT HTF interval={}: {} nến đã đóng cửa.",
            interval,
            candles.len()
        );
        Ok(candles)
    }

    /// Build a snapshot from the read-only Exness MT5 JSON feed.
    fn exness_snapshot(
        &self,
        timeframe: Timeframe,
        limit: usize,
        broker_price_offset: f64,
    ) -> Option<MarketFeedSnapshot> {
        let file = load_exness_file(&self.exness_path)?;
        let now = Utc::now();
        if !file.is_fresh(now, EXNESS_MAX_AGE_SECS) {
            warn!(
                "⚠️ [ExnessFeed] Dữ liệu cũ {}s (max {}s) — bỏ qua.",
                file.age_secs(now).unwrap_or(i64::MAX),
                EXNESS_MAX_AGE_SECS
            );
            return None;
        }
        let label = timeframe_label(timeframe);
        let candles = exness_candles(&file, label, timeframe, limit);
        if candles.is_empty() {
            warn!("⚠️ [ExnessFeed] Không có nến {} trong file.", label);
            return None;
        }
        let live_price = file
            .tick
            .as_ref()
            .and_then(|tick| tick.mid())
            .or_else(|| candles.last().map(|candle| candle.close));
        let mut snapshot = MarketFeedSnapshot {
            candles,
            source: SOURCE_EXNESS.to_string(),
            live_price,
            volume_source: VOLUME_SOURCE_EXNESS_TICK.to_string(),
        };
        Self::apply_broker_offset(&mut snapshot, broker_price_offset);
        info!(
            "📡 [MarketFeed] Exness {} {}: {} nến đã đóng, live={:?}, server={}.",
            label,
            file.symbol,
            snapshot.candles.len(),
            snapshot.live_price,
            file.server
        );
        Some(snapshot)
    }

    fn exness_htf_candles(
        &self,
        label: &str,
        timeframe: Timeframe,
        limit: usize,
    ) -> Option<Vec<Candle>> {
        let file = load_exness_file(&self.exness_path)?;
        if !file.is_fresh(Utc::now(), EXNESS_MAX_AGE_SECS) {
            warn!("⚠️ [ExnessFeed] HTF {} stale — bỏ qua.", label);
            return None;
        }
        let candles = exness_candles(&file, label, timeframe, limit);
        if candles.is_empty() {
            None
        } else {
            Some(candles)
        }
    }

    /// Kéo ảnh chụp thị trường: nến đã đóng, giá live và nguồn thực tế.
    ///
    /// `preferred_source` quyết định provider được thử trước. Nếu provider ưu tiên
    /// lỗi và provider còn lại khả dụng, tự động fallback sang provider kia.
    /// Chỉ khi cả hai đều thất bại mới trả `FALLBACK_SEED`.
    pub async fn fetch_market_snapshot(
        &self,
        timeframe: Timeframe,
        limit: usize,
        preferred_source: &str,
        broker_price_offset: f64,
    ) -> MarketFeedSnapshot {
        let limit = limit.max(5);
        let preferred = preferred_source.trim().to_uppercase();
        let prefer_oanda = preferred == "OANDA";
        let mut prefer_twelve = preferred == "TWELVEDATA";
        let has_twelve = self.twelve_data_api_key.is_some();

        if preferred == "EXNESS" {
            if let Some(snapshot) = self.exness_snapshot(timeframe, limit, broker_price_offset) {
                return snapshot;
            }
            warn!("⚠️ Exness feed không khả dụng/cũ — fallback sang nguồn khác.");
            prefer_twelve = has_twelve;
        }

        if prefer_oanda {
            if self.oanda_api_token.is_some() {
                if let Some(mut oanda) = self.fetch_oanda_snapshot(timeframe, limit).await {
                    Self::apply_broker_offset(&mut oanda, broker_price_offset);
                    return oanda;
                }
                warn!("⚠️ OANDA không khả dụng, fallback sang nguồn khác.");
            } else {
                warn!("⚠️ OANDA chưa có OANDA_API_TOKEN, fallback sang nguồn khác.");
            }
            prefer_twelve = has_twelve;
        }

        let mut snapshot = if prefer_twelve && has_twelve {
            // TwelveData only has OHLC. Yahoo GC=F volume is preferred; Binance
            // PAXG is the fallback, then NONE.
            let yahoo_enabled = self.yahoo_futures_volume_enabled();
            let (twelve, binance, yahoo) = tokio::join!(
                self.fetch_twelve_snapshot(timeframe, limit),
                self.fetch_binance_snapshot(timeframe, limit),
                async {
                    if yahoo_enabled {
                        self.fetch_yahoo_volume(timeframe, limit).await
                    } else {
                        None
                    }
                }
            );
            match twelve {
                Some(mut twelve) => {
                    let yahoo_matched = yahoo
                        .as_deref()
                        .filter(|_| !twelve.candles.is_empty())
                        .map(|yahoo| {
                            let (matches, total) =
                                Self::recent_volume_matches(&twelve.candles, yahoo, 20);
                            if total > 0 && matches * 2 >= total {
                                Self::merge_volume_proxy(&mut twelve.candles, yahoo);
                                true
                            } else {
                                warn!(
                                    "⚠️ Yahoo GC=F chỉ khớp {}/{} nến gần nhất — fallback PAXG.",
                                    matches, total
                                );
                                false
                            }
                        })
                        .unwrap_or(false);

                    let binance_matched = if yahoo_matched {
                        false
                    } else {
                        binance
                            .as_ref()
                            .map(|binance| {
                                Self::merge_volume_proxy(&mut twelve.candles, &binance.candles)
                            })
                            .unwrap_or(false)
                    };

                    twelve.volume_source = if yahoo_matched {
                        VOLUME_SOURCE_YAHOO_FUTURES_PROXY.to_string()
                    } else if binance_matched {
                        VOLUME_SOURCE_BINANCE_PAXG_PROXY.to_string()
                    } else {
                        VOLUME_SOURCE_NONE.to_string()
                    };

                    if yahoo_matched {
                        info!(
                            "🔊 [MarketFeed] Đã ghép volume Yahoo GC=F vào {} nến TwelveData.",
                            twelve.candles.len()
                        );
                    } else if binance_matched {
                        info!(
                            "🔊 [MarketFeed] Đã ghép volume Binance PAXG vào {} nến TwelveData.",
                            twelve.candles.len()
                        );
                    } else {
                        warn!("⚠️ TwelveData không khớp timestamp với Yahoo/Binance — volume = 0.");
                    }
                    Some(twelve)
                }
                None => {
                    warn!("⚠️ TwelveData không khả dụng, chuyển sang Binance PAXGUSDT.");
                    binance
                }
            }
        } else {
            let binance = self.fetch_binance_snapshot(timeframe, limit).await;
            if binance.is_some() {
                binance
            } else if has_twelve {
                warn!("⚠️ Binance không khả dụng, chuyển sang TwelveData XAU/USD.");
                self.fetch_twelve_snapshot(timeframe, limit).await
            } else {
                None
            }
        };

        match snapshot.take() {
            Some(mut data) => {
                Self::apply_broker_offset(&mut data, broker_price_offset);
                data
            }
            None => {
                warn!("⚠️ Cả Binance và TwelveData đều thất bại, dùng nến fallback offline.");
                MarketFeedSnapshot::fallback(limit, timeframe)
            }
        }
    }

    async fn fetch_binance_snapshot(
        &self,
        timeframe: Timeframe,
        limit: usize,
    ) -> Option<MarketFeedSnapshot> {
        match self.fetch_binance(timeframe, limit).await {
            Ok(snapshot) if !snapshot.candles.is_empty() => Some(snapshot),
            Ok(_) => {
                warn!("⚠️ Binance trả về rỗng.");
                None
            }
            Err(err) => {
                warn!("⚠️ Không kết nối được Binance ({}).", err);
                None
            }
        }
    }

    async fn fetch_twelve_snapshot(
        &self,
        timeframe: Timeframe,
        limit: usize,
    ) -> Option<MarketFeedSnapshot> {
        let api_key = self.twelve_data_api_key.as_deref()?;
        let cache_label = twelve_cache_label(timeframe);
        if let Some(cached) = self.cached_twelve_snapshot(cache_label).await {
            return Some(cached);
        }

        match self.fetch_twelve_data(timeframe, limit, api_key).await {
            Ok(snapshot) if !snapshot.candles.is_empty() => {
                self.store_twelve_snapshot(cache_label, &snapshot).await;
                Some(snapshot)
            }
            Ok(_) => {
                warn!("⚠️ TwelveData trả về rỗng.");
                None
            }
            Err(err) => {
                warn!("⚠️ TwelveData lỗi ({}).", err);
                None
            }
        }
    }

    async fn fetch_twelve_daily_snapshot(&self, limit: usize) -> Option<MarketFeedSnapshot> {
        let api_key = self.twelve_data_api_key.as_deref()?;
        let cache_label = "D1";
        if let Some(cached) = self.cached_twelve_snapshot(cache_label).await {
            return Some(cached);
        }

        match self
            .fetch_twelve_data_interval("1day", Timeframe::H4, DAY_MS, limit, api_key)
            .await
        {
            Ok(snapshot) if !snapshot.candles.is_empty() => {
                self.store_twelve_snapshot(cache_label, &snapshot).await;
                Some(snapshot)
            }
            Ok(_) => {
                warn!("⚠️ TwelveData D1 trả về rỗng.");
                None
            }
            Err(err) => {
                warn!("⚠️ TwelveData D1 lỗi ({}).", err);
                None
            }
        }
    }

    async fn fetch_oanda_snapshot(
        &self,
        timeframe: Timeframe,
        limit: usize,
    ) -> Option<MarketFeedSnapshot> {
        let token = self.oanda_api_token.as_deref()?;
        let cache_label = twelve_cache_label(timeframe);
        if let Some(cached) = self.cached_oanda_snapshot(cache_label).await {
            return Some(cached);
        }

        match self
            .fetch_oanda_data(
                oanda_granularity(timeframe),
                timeframe,
                timeframe_millis(timeframe),
                limit,
                token,
            )
            .await
        {
            Ok(snapshot) if !snapshot.candles.is_empty() => {
                self.store_oanda_snapshot(cache_label, &snapshot).await;
                Some(snapshot)
            }
            Ok(_) => {
                warn!("⚠️ OANDA trả về rỗng.");
                None
            }
            Err(err) => {
                warn!("⚠️ OANDA lỗi ({}).", err);
                None
            }
        }
    }

    async fn fetch_oanda_daily_snapshot(&self, limit: usize) -> Option<MarketFeedSnapshot> {
        let token = self.oanda_api_token.as_deref()?;
        let cache_label = "D1";
        if let Some(cached) = self.cached_oanda_snapshot(cache_label).await {
            return Some(cached);
        }

        match self
            .fetch_oanda_data("D", Timeframe::H4, DAY_MS, limit, token)
            .await
        {
            Ok(snapshot) if !snapshot.candles.is_empty() => {
                self.store_oanda_snapshot(cache_label, &snapshot).await;
                Some(snapshot)
            }
            Ok(_) => {
                warn!("⚠️ OANDA D1 trả về rỗng.");
                None
            }
            Err(err) => {
                warn!("⚠️ OANDA D1 lỗi ({}).", err);
                None
            }
        }
    }

    async fn fetch_oanda_data(
        &self,
        granularity: &str,
        timeframe_tag: Timeframe,
        step_ms: i64,
        limit: usize,
        token: &str,
    ) -> Result<MarketFeedSnapshot, String> {
        let url = format!(
            "{}/v3/instruments/XAU_USD/candles?granularity={}&count={}&price=M&order=A",
            self.oanda_base_url,
            granularity,
            limit + 1
        );
        let response = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("OANDA request error: {}", e.without_url()))?;

        if !response.status().is_success() {
            return Err(format!("OANDA HTTP {}", response.status()));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("OANDA JSON parse error: {}", e.without_url()))?;
        if let Some(message) = payload.get("errorMessage").and_then(|value| value.as_str()) {
            return Err(format!("OANDA API error: {}", message));
        }

        let (candles, live_price) = Self::parse_oanda_candles_with_now(
            &payload,
            timeframe_tag,
            step_ms,
            Utc::now().timestamp_millis(),
            limit,
        );
        info!(
            "📡 [MarketFeed] OANDA XAU/USD granularity={}: {} nến đã đóng cửa, live={:?}.",
            granularity,
            candles.len(),
            live_price
        );
        Ok(MarketFeedSnapshot {
            candles,
            source: SOURCE_OANDA_XAUUSD.to_string(),
            live_price,
            volume_source: VOLUME_SOURCE_OANDA_TICK.to_string(),
        })
    }

    /// Cộng offset broker vào toàn bộ OHLC của nến đã đóng và giá live, làm tròn 2 chữ số.
    pub(crate) fn apply_broker_offset(snapshot: &mut MarketFeedSnapshot, broker_price_offset: f64) {
        let offset = if broker_price_offset.is_finite() {
            broker_price_offset
        } else {
            0.0
        };
        Self::shift_snapshot_prices(snapshot, offset);
    }

    /// Dịch toàn bộ OHLC đã đóng + giá live theo basis spot (PAXG → XAU/USD),
    /// làm tròn 2 chữ số. Basis không hữu hạn hoặc bằng 0 là no-op.
    pub fn shift_snapshot_prices(snapshot: &mut MarketFeedSnapshot, basis: f64) {
        if !basis.is_finite() || basis == 0.0 {
            return;
        }

        for candle in &mut snapshot.candles {
            candle.open = round2(candle.open + basis);
            candle.high = round2(candle.high + basis);
            candle.low = round2(candle.low + basis);
            candle.close = round2(candle.close + basis);
        }
        snapshot.live_price = snapshot.live_price.map(|price| round2(price + basis));
    }

    async fn fetch_binance(
        &self,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<MarketFeedSnapshot, String> {
        let interval = binance_interval(timeframe);
        // limit + 1 để luôn có cây nến vừa đóng cửa ngay cả khi API trả cây đang chạy.
        let url = format!(
            "https://api.binance.com/api/v3/klines?symbol=PAXGUSDT&interval={}&limit={}",
            interval,
            limit + 1
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Binance request error: {}", e.without_url()))?;

        if !response.status().is_success() {
            return Err(format!("Binance HTTP {}", response.status()));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("Binance JSON parse error: {}", e.without_url()))?;

        let (candles, live_price) = Self::parse_binance_klines_snapshot_with_now(
            &payload,
            timeframe,
            Utc::now().timestamp_millis(),
        );

        info!(
            "📡 [MarketFeed] Binance PAXGUSDT {}: {} nến đã đóng cửa, live={:?}.",
            timeframe,
            candles.len(),
            live_price
        );
        Ok(MarketFeedSnapshot {
            candles,
            source: "BINANCE_PAXG".to_string(),
            live_price,
            volume_source: VOLUME_SOURCE_BINANCE_PAXG.to_string(),
        })
    }

    async fn fetch_twelve_data(
        &self,
        timeframe: Timeframe,
        limit: usize,
        api_key: &str,
    ) -> Result<MarketFeedSnapshot, String> {
        self.fetch_twelve_data_interval(
            twelve_interval(timeframe),
            timeframe,
            timeframe_millis(timeframe),
            limit,
            api_key,
        )
        .await
    }

    async fn fetch_twelve_data_interval(
        &self,
        interval: &str,
        timeframe_tag: Timeframe,
        step_ms: i64,
        limit: usize,
        api_key: &str,
    ) -> Result<MarketFeedSnapshot, String> {
        let url = format!(
            "https://api.twelvedata.com/time_series?symbol=XAU/USD&interval={}&outputsize={}&timezone=UTC&order=ASC&apikey={}",
            interval,
            limit + 1,
            api_key
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("TwelveData request error: {}", e.without_url()))?;

        if !response.status().is_success() {
            return Err(format!("TwelveData HTTP {}", response.status()));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("TwelveData JSON parse error: {}", e.without_url()))?;

        if let Some(message) = payload.get("message").and_then(|v| v.as_str()) {
            return Err(format!("TwelveData API error: {}", message));
        }

        let (candles, live_price) = Self::parse_twelve_data_snapshot_with_step(
            &payload,
            timeframe_tag,
            step_ms,
            Utc::now().timestamp_millis(),
            limit,
        );

        info!(
            "📡 [MarketFeed] TwelveData XAU/USD interval={}: {} nến đã đóng cửa, live={:?}.",
            interval,
            candles.len(),
            live_price
        );
        Ok(MarketFeedSnapshot {
            candles,
            source: "TWELVEDATA".to_string(),
            live_price,
            volume_source: VOLUME_SOURCE_NONE.to_string(),
        })
    }

    /// Parse mảng klines Binance và loại bỏ cây nến chưa đóng cửa.
    #[cfg(test)]
    pub(crate) fn parse_binance_klines_with_now(
        payload: &Value,
        timeframe: Timeframe,
        now_ms: i64,
    ) -> Vec<Candle> {
        Self::parse_binance_klines_snapshot_with_now(payload, timeframe, now_ms).0
    }

    /// Parse Binance klines: trả về nến đã đóng + close của cây đang chạy làm live price.
    pub(crate) fn parse_binance_klines_snapshot_with_now(
        payload: &Value,
        timeframe: Timeframe,
        now_ms: i64,
    ) -> (Vec<Candle>, Option<f64>) {
        let Some(rows) = payload.as_array() else {
            return (Vec::new(), None);
        };

        let mut candles = Vec::with_capacity(rows.len());
        let mut live_price: Option<f64> = None;
        for row in rows {
            let Some(items) = row.as_array() else {
                continue;
            };
            if items.len() < 7 {
                continue;
            }

            let Some(open_time) = value_to_i64(&items[0]) else {
                continue;
            };
            let Some(close_time) = value_to_i64(&items[6]) else {
                continue;
            };

            let parsed = (
                value_to_f64(&items[1]),
                value_to_f64(&items[2]),
                value_to_f64(&items[3]),
                value_to_f64(&items[4]),
                value_to_f64(&items[5]),
            );
            let (Some(open), Some(high), Some(low), Some(close), Some(volume)) = parsed else {
                continue;
            };

            if !is_finite_ohlcv(open, high, low, close, volume) {
                continue;
            }

            // Binance kline field 9 = taker buy base asset volume.
            let taker_buy_volume = items
                .get(9)
                .and_then(value_to_f64)
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(round2)
                .unwrap_or(0.0);

            // Cây nến đang chạy: close_time nằm trong tương lai -> chỉ lấy close làm live price.
            if close_time > now_ms {
                live_price = Some(close);
                continue;
            }

            candles.push(Candle {
                timestamp: open_time,
                open,
                high,
                low,
                close,
                volume,
                taker_buy_volume,
                timeframe,
            });
        }

        let live_price = live_price.or_else(|| candles.last().map(|candle| candle.close));
        (candles, live_price)
    }

    /// Parse TwelveData time_series values (mảng mới nhất đứng đầu).
    #[cfg(test)]
    pub(crate) fn parse_twelve_data_values_with_now(
        payload: &Value,
        timeframe: Timeframe,
        now_ms: i64,
        limit: usize,
    ) -> Vec<Candle> {
        Self::parse_twelve_data_snapshot_with_now(payload, timeframe, now_ms, limit).0
    }

    /// Parse TwelveData: trả về nến đã đóng + close của cây đang chạy làm live price.
    #[cfg(test)]
    pub(crate) fn parse_twelve_data_snapshot_with_now(
        payload: &Value,
        timeframe: Timeframe,
        now_ms: i64,
        limit: usize,
    ) -> (Vec<Candle>, Option<f64>) {
        Self::parse_twelve_data_snapshot_with_step(
            payload,
            timeframe,
            timeframe_millis(timeframe),
            now_ms,
            limit,
        )
    }

    /// Parser dùng chung cho intraday và D1 (`step_ms` = 24 giờ).
    /// Parse OANDA v3 candles. `complete=true` candles become closed candles;
    /// the incomplete candle's mid close is returned as live price. OANDA only
    /// exposes tick volume, so `taker_buy_volume` is always 0.
    pub(crate) fn parse_oanda_candles_with_now(
        payload: &Value,
        timeframe: Timeframe,
        step_ms: i64,
        now_ms: i64,
        limit: usize,
    ) -> (Vec<Candle>, Option<f64>) {
        let Some(rows) = payload.get("candles").and_then(|value| value.as_array()) else {
            return (Vec::new(), None);
        };

        let mut candles = Vec::new();
        let mut live_price = None;
        for row in rows {
            let Some(mid) = row.get("mid") else {
                continue;
            };
            let (Some(open), Some(high), Some(low), Some(close)) = (
                mid.get("o").and_then(value_to_f64),
                mid.get("h").and_then(value_to_f64),
                mid.get("l").and_then(value_to_f64),
                mid.get("c").and_then(value_to_f64),
            ) else {
                continue;
            };
            let Some(time) = row
                .get("time")
                .and_then(|value| value.as_str())
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.timestamp_millis())
            else {
                continue;
            };

            let complete = row
                .get("complete")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if !complete {
                live_price = Some(round2(close));
                continue;
            }
            // Defensive: ignore a candle whose close time is still in the future.
            if time.saturating_add(step_ms) > now_ms {
                continue;
            }
            let volume = row
                .get("volume")
                .and_then(value_to_f64)
                .unwrap_or(0.0)
                .max(0.0);
            candles.push(Candle {
                timestamp: time,
                open: round2(open),
                high: round2(high),
                low: round2(low),
                close: round2(close),
                volume,
                taker_buy_volume: 0.0,
                timeframe,
            });
        }

        candles.sort_by_key(|candle| candle.timestamp);
        if candles.len() > limit {
            candles = candles.split_off(candles.len() - limit);
        }
        (candles, live_price)
    }

    pub(crate) fn parse_twelve_data_snapshot_with_step(
        payload: &Value,
        timeframe: Timeframe,
        step_ms: i64,
        now_ms: i64,
        limit: usize,
    ) -> (Vec<Candle>, Option<f64>) {
        let Some(rows) = payload.get("values").and_then(|v| v.as_array()) else {
            return (Vec::new(), None);
        };

        let mut candles = Vec::with_capacity(rows.len());
        let mut live_price: Option<f64> = None;

        for row in rows {
            let Some(datetime) = row.get("datetime").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(timestamp) = parse_twelve_data_datetime(datetime) else {
                continue;
            };

            let parsed = (
                value_to_f64(row.get("open").unwrap_or(&Value::Null)),
                value_to_f64(row.get("high").unwrap_or(&Value::Null)),
                value_to_f64(row.get("low").unwrap_or(&Value::Null)),
                value_to_f64(row.get("close").unwrap_or(&Value::Null)),
            );
            let (Some(open), Some(high), Some(low), Some(close)) = parsed else {
                continue;
            };

            if !is_finite_ohlc(open, high, low, close) {
                continue;
            }

            // TwelveData time_series không có volume: thiếu/không hợp lệ -> 0.0.
            let volume = row
                .get("volume")
                .and_then(value_to_f64)
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(0.0);

            // Cây nến chưa đóng cửa: thời điểm đóng = open + step > now.
            if timestamp + step_ms > now_ms {
                if live_price.is_none() {
                    live_price = Some(close);
                }
                continue;
            }

            candles.push(Candle {
                timestamp,
                open,
                high,
                low,
                close,
                volume,
                taker_buy_volume: 0.0,
                timeframe,
            });
        }

        candles.sort_by_key(|candle| candle.timestamp);
        if candles.len() > limit {
            let keep_from = candles.len() - limit;
            candles.drain(0..keep_from);
        }

        let live_price = live_price.or_else(|| candles.last().map(|candle| candle.close));
        (candles, live_price)
    }

    /// Sinh nến mẫu offline (deterministic, không panic) quanh vùng giá 2680 USD.
    pub fn generate_fallback_gold_candles(limit: usize, timeframe: Timeframe) -> Vec<Candle> {
        let limit = limit.max(5);
        let step_ms = timeframe_millis(timeframe);
        let now_ms = Utc::now().timestamp_millis();
        // Open time của cây nến M15/H1/H4 vừa đóng cửa gần nhất.
        let last_closed_open = (now_ms / step_ms) * step_ms - step_ms;

        let mut candles = Vec::with_capacity(limit);
        let mut price = 2680.0;

        for i in 0..limit {
            let timestamp = last_closed_open - ((limit - 1 - i) as i64 * step_ms);
            let delta = ((i as f64 * 0.4).sin() * 2.8) + (((i % 7) as f64 - 3.0) * 1.2);
            let open = price;
            let close = (price + delta).max(2600.0);
            let high = open.max(close) + 1.2 + ((i % 5) as f64 * 0.5);
            let low = open.min(close) - 1.0 - ((i % 4) as f64 * 0.6);
            let base_volume = 1200.0 + ((i % 11) as f64 * 180.0);
            let volume = if i == limit - 1 {
                base_volume * 1.9
            } else {
                base_volume
            };

            candles.push(Candle {
                timestamp,
                open: round2(open),
                high: round2(high),
                low: round2(low),
                close: round2(close),
                volume: round2(volume),
                taker_buy_volume: 0.0,
                timeframe,
            });

            price = close;
        }

        candles
    }
}

impl Default for MarketDataFeed {
    fn default() -> Self {
        Self::new()
    }
}

fn timeframe_millis(timeframe: Timeframe) -> i64 {
    match timeframe {
        Timeframe::M15 => 15 * 60 * 1000,
        Timeframe::H1 => 60 * 60 * 1000,
        Timeframe::H4 => 4 * 60 * 60 * 1000,
    }
}

fn binance_interval(timeframe: Timeframe) -> &'static str {
    match timeframe {
        Timeframe::M15 => "15m",
        Timeframe::H1 => "1h",
        Timeframe::H4 => "4h",
    }
}

fn twelve_interval(timeframe: Timeframe) -> &'static str {
    match timeframe {
        Timeframe::M15 => "15min",
        Timeframe::H1 => "1h",
        Timeframe::H4 => "4h",
    }
}

fn yahoo_interval(timeframe: Timeframe) -> &'static str {
    match timeframe {
        Timeframe::M15 => "15m",
        Timeframe::H1 | Timeframe::H4 => "1h",
    }
}

fn yahoo_lookback_days(timeframe: Timeframe, limit: usize) -> i64 {
    let bars = limit.max(5) as i64;
    match timeframe {
        Timeframe::M15 => (bars * 15 / (60 * 24)) + 4,
        Timeframe::H1 => (bars * 60 / (60 * 24)) + 7,
        Timeframe::H4 => (bars * 4 * 60 / (60 * 24)) + 14,
    }
}

/// Yahoo's daily bar opens at 05:00/06:00 UTC (00:00 ET). Normalise it to the
/// UTC midnight of the same calendar date so it matches TwelveData D1 candles.
fn yahoo_daily_timestamp(timestamp_ms: i64) -> i64 {
    timestamp_ms - timestamp_ms.rem_euclid(DAY_MS)
}

/// Yahoo GC=F and OANDA tick volume have no taker buy/sell split, so delta/CVD
/// must not be derived from them.
pub fn volume_source_has_buy_sell_delta(source: &str) -> bool {
    !source.eq_ignore_ascii_case(VOLUME_SOURCE_OANDA_TICK)
        && !source.eq_ignore_ascii_case(VOLUME_SOURCE_EXNESS_TICK)
        && !source.eq_ignore_ascii_case(VOLUME_SOURCE_YAHOO_FUTURES_PROXY)
        && !source.eq_ignore_ascii_case(VOLUME_SOURCE_NONE)
}

fn oanda_granularity(timeframe: Timeframe) -> &'static str {
    match timeframe {
        Timeframe::M15 => "M15",
        Timeframe::H1 => "H1",
        Timeframe::H4 => "H4",
    }
}

fn twelve_cache_label(timeframe: Timeframe) -> &'static str {
    match timeframe {
        Timeframe::M15 => "M15",
        Timeframe::H1 => "H1",
        Timeframe::H4 => "H4",
    }
}

/// TTL cache TwelveData theo khung: M15 5 phút, H1/H4 15 phút, D1 1 giờ.
/// Tổng usage ~504 request/ngày, an toàn cho free tier 800/ngày.
fn twelve_cache_ttl(label: &str) -> Duration {
    match label.trim().to_uppercase().as_str() {
        // The scanner needs the M15 candle that just closed. A 5-minute cache
        // made every M15 close 3-5 minutes old, so the freshness gate rejected
        // it and no confirmed signal / setup-watching alert could ever fire.
        // 45s guarantees the 60s feed tick refetches on every pass while still
        // staying well inside the free-tier budget (1 request/minute).
        "M15" => Duration::from_secs(45),
        "H1" | "H4" => Duration::from_secs(900),
        "D1" => Duration::from_secs(3600),
        _ => Duration::from_secs(300),
    }
}

fn value_to_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
}

fn value_to_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn is_finite_ohlcv(open: f64, high: f64, low: f64, close: f64, volume: f64) -> bool {
    [open, high, low, close, volume]
        .iter()
        .all(|value| value.is_finite())
        && high >= low
        && volume >= 0.0
}

fn is_finite_ohlc(open: f64, high: f64, low: f64, close: f64) -> bool {
    [open, high, low, close]
        .iter()
        .all(|value| value.is_finite())
        && high >= low
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn parse_twelve_data_datetime(value: &str) -> Option<i64> {
    // TwelveData thường trả "YYYY-MM-DD HH:MM:SS" (UTC), đôi khi có ISO RFC3339.
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc).timestamp_millis());
    }

    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M"))
        .ok();
    if let Some(naive) = naive {
        return Some(Utc.from_utc_datetime(&naive).timestamp_millis());
    }

    // D1 time_series trả ngày thuần "YYYY-MM-DD" (UTC).
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let naive = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&naive).timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_candle(timestamp: i64, volume: f64, taker_buy_volume: f64) -> Candle {
        Candle {
            timestamp,
            open: 2000.0,
            high: 2001.0,
            low: 1999.0,
            close: 2000.5,
            volume,
            taker_buy_volume,
            timeframe: Timeframe::M15,
        }
    }

    #[test]
    fn test_parse_binance_klines_and_strip_unclosed_candle() {
        let payload = json!([
            [
                1_700_000_000_000_i64,
                "2000.10",
                "2010.50",
                "1995.00",
                "2005.25",
                "123.45",
                1_700_000_899_999_i64,
                "0",
                10,
                "0",
                "0",
                "0"
            ],
            [
                1_700_000_900_000_i64,
                "2005.25",
                "2015.00",
                "2000.00",
                "2012.75",
                "150.10",
                1_700_001_799_999_i64,
                "0",
                10,
                "75.55",
                "0",
                "0"
            ]
        ]);

        // now sau close_time của cả 2 nến -> giữ nguyên.
        let candles = MarketDataFeed::parse_binance_klines_with_now(
            &payload,
            Timeframe::M15,
            1_700_001_800_000,
        );
        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].timestamp, 1_700_000_000_000);
        assert!((candles[1].close - 2012.75).abs() < 1e-9);
        assert!((candles[1].volume - 150.10).abs() < 1e-9);
        assert!((candles[1].taker_buy_volume - 75.55).abs() < 1e-9);
        assert_eq!(candles[1].timeframe, Timeframe::M15);

        // now rơi vào cây nến thứ 2 -> cây chưa đóng cửa bị loại bỏ.
        let closed_only = MarketDataFeed::parse_binance_klines_with_now(
            &payload,
            Timeframe::M15,
            1_700_001_000_000,
        );
        assert_eq!(closed_only.len(), 1);
        assert_eq!(closed_only[0].timestamp, 1_700_000_000_000);
    }

    #[test]
    fn test_parse_binance_klines_defaults_missing_taker_buy_volume() {
        let payload = json!([[
            1_700_000_000_000_i64,
            "2000.10",
            "2010.50",
            "1995.00",
            "2005.25",
            "123.45",
            1_700_000_899_999_i64
        ]]);

        let candles =
            MarketDataFeed::parse_binance_klines_with_now(&payload, Timeframe::M15, i64::MAX);
        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].taker_buy_volume, 0.0);
    }

    #[test]
    fn test_parse_binance_klines_skips_malformed_rows() {
        let payload = json!([
            ["bad", "1", "2", "0.5", "1.5", "10", 1_700_000_899_999_i64],
            [1_700_000_900_000_i64, "1", "2", "0.5", "1.5", "10"]
        ]);

        let candles =
            MarketDataFeed::parse_binance_klines_with_now(&payload, Timeframe::M15, i64::MAX);
        assert!(candles.is_empty());
    }

    #[test]
    fn test_parse_twelve_data_values_and_strip_current_candle() {
        let payload = json!({
            "values": [
                {
                    "datetime": "2026-11-09 12:15:00",
                    "open": "2010.0",
                    "high": "2020.0",
                    "low": "2005.0",
                    "close": "2015.0",
                    "volume": "120"
                },
                {
                    "datetime": "2026-11-09 12:00:00",
                    "open": "2000.0",
                    "high": "2012.0",
                    "low": "1995.0",
                    "close": "2010.0",
                    "volume": "100"
                }
            ]
        });

        let open_1200 = Utc
            .with_ymd_and_hms(2026, 11, 9, 12, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp_millis();
        let open_1215 = Utc
            .with_ymd_and_hms(2026, 11, 9, 12, 15, 0)
            .single()
            .expect("valid datetime")
            .timestamp_millis();

        // Tại 12:20 UTC: nến 12:00 đã đóng, nến 12:15 còn đang chạy.
        let now = open_1215 + 5 * 60 * 1000;
        let candles =
            MarketDataFeed::parse_twelve_data_values_with_now(&payload, Timeframe::M15, now, 100);

        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].timestamp, open_1200);
        assert!((candles[0].close - 2010.0).abs() < 1e-9);
        assert!((candles[0].volume - 100.0).abs() < 1e-9);
    }

    #[test]
    fn test_parse_twelve_data_accepts_missing_volume() {
        let payload = json!({
            "values": [
                {
                    "datetime": "2026-11-09 12:00:00",
                    "open": "2000.0",
                    "high": "2012.0",
                    "low": "1995.0",
                    "close": "2010.0"
                }
            ]
        });

        let open_1200 = Utc
            .with_ymd_and_hms(2026, 11, 9, 12, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp_millis();
        let candles = MarketDataFeed::parse_twelve_data_values_with_now(
            &payload,
            Timeframe::M15,
            open_1200 + 15 * 60 * 1000 + 1,
            100,
        );

        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].volume, 0.0);
        assert_eq!(candles[0].taker_buy_volume, 0.0);
    }

    #[test]
    fn test_parse_twelve_data_keeps_present_volume() {
        let payload = json!({
            "values": [
                {
                    "datetime": "2026-11-09 12:00:00",
                    "open": "2000.0",
                    "high": "2012.0",
                    "low": "1995.0",
                    "close": "2010.0",
                    "volume": "123.45"
                }
            ]
        });

        let open_1200 = Utc
            .with_ymd_and_hms(2026, 11, 9, 12, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp_millis();
        let candles = MarketDataFeed::parse_twelve_data_values_with_now(
            &payload,
            Timeframe::M15,
            open_1200 + 15 * 60 * 1000 + 1,
            100,
        );

        assert_eq!(candles.len(), 1);
        assert!((candles[0].volume - 123.45).abs() < 1e-9);
        assert_eq!(candles[0].taker_buy_volume, 0.0);
    }

    #[test]
    fn test_parse_twelve_data_daily_uses_24h_step_and_date_only() {
        let payload = json!({
            "values": [
                {
                    "datetime": "2026-11-10",
                    "open": "2010.0",
                    "high": "2020.0",
                    "low": "2005.0",
                    "close": "2015.0"
                },
                {
                    "datetime": "2026-11-09",
                    "open": "2000.0",
                    "high": "2012.0",
                    "low": "1995.0",
                    "close": "2010.0"
                }
            ]
        });

        let now = Utc
            .with_ymd_and_hms(2026, 11, 10, 12, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp_millis();
        let (candles, live_price) = MarketDataFeed::parse_twelve_data_snapshot_with_step(
            &payload,
            Timeframe::H4,
            DAY_MS,
            now,
            100,
        );

        assert_eq!(candles.len(), 1);
        assert_eq!(
            candles[0].timestamp,
            Utc.with_ymd_and_hms(2026, 11, 9, 0, 0, 0)
                .single()
                .expect("valid datetime")
                .timestamp_millis()
        );
        assert_eq!(candles[0].volume, 0.0);
        assert_eq!(live_price, Some(2015.0));
    }

    #[test]
    fn test_twelve_cache_ttl_matches_free_tier_budget() {
        // M15 must expire before the next 60s feed tick, otherwise the freshly
        // closed candle is served from cache and the freshness gate rejects it.
        assert_eq!(twelve_cache_ttl("M15"), Duration::from_secs(45));
        assert_eq!(twelve_cache_ttl("H1"), Duration::from_secs(900));
        assert_eq!(twelve_cache_ttl("H4"), Duration::from_secs(900));
        assert_eq!(twelve_cache_ttl("D1"), Duration::from_secs(3600));
        assert_eq!(twelve_cache_ttl("unknown"), Duration::from_secs(300));
    }

    #[test]
    fn test_merge_volume_proxy_matches_by_timestamp() {
        let mut price_candles = vec![test_candle(1, 0.0, 0.0), test_candle(2, 0.0, 0.0)];
        let volume_candles = vec![test_candle(2, 150.0, 75.0), test_candle(3, 999.0, 0.0)];

        assert!(MarketDataFeed::merge_volume_proxy(
            &mut price_candles,
            &volume_candles
        ));
        assert_eq!(price_candles[0].volume, 0.0);
        assert_eq!(price_candles[1].volume, 150.0);
        assert_eq!(price_candles[1].taker_buy_volume, 75.0);
    }

    #[test]
    fn test_merge_volume_proxy_returns_false_without_matching_timestamp() {
        let mut price_candles = vec![test_candle(1, 10.0, 5.0)];
        let volume_candles = vec![test_candle(2, 150.0, 75.0)];

        assert!(!MarketDataFeed::merge_volume_proxy(
            &mut price_candles,
            &volume_candles
        ));
        assert_eq!(price_candles[0].volume, 10.0);
        assert_eq!(price_candles[0].taker_buy_volume, 5.0);
    }

    #[test]
    fn test_fallback_candles_are_ordered_and_closed() {
        let candles = MarketDataFeed::generate_fallback_gold_candles(50, Timeframe::M15);
        assert_eq!(candles.len(), 50);

        let now = Utc::now().timestamp_millis();
        for (index, candle) in candles.iter().enumerate() {
            assert!(candle.high >= candle.low);
            assert!(candle.timestamp + 15 * 60 * 1000 <= now);
            if index > 0 {
                assert!(candle.timestamp > candles[index - 1].timestamp);
            }
        }
    }

    #[test]
    fn test_binance_snapshot_exposes_live_price_from_unclosed_candle() {
        let payload = json!([
            [
                1_700_000_000_000_i64,
                "2000.10",
                "2010.50",
                "1995.00",
                "2005.25",
                "123.45",
                1_700_000_899_999_i64
            ],
            [
                1_700_000_900_000_i64,
                "2005.25",
                "2015.00",
                "2000.00",
                "2012.75",
                "150.10",
                1_700_001_799_999_i64
            ]
        ]);

        // now rơi vào cây thứ 2 -> chỉ cây 1 đóng cửa, live = close cây 2.
        let (candles, live_price) = MarketDataFeed::parse_binance_klines_snapshot_with_now(
            &payload,
            Timeframe::M15,
            1_700_001_000_000,
        );
        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].timestamp, 1_700_000_000_000);
        assert_eq!(live_price, Some(2012.75));
    }

    #[test]
    fn test_twelve_data_snapshot_exposes_live_price_from_forming_candle() {
        let payload = json!({
            "values": [
                {
                    "datetime": "2026-11-09 12:15:00",
                    "open": "2010.0",
                    "high": "2020.0",
                    "low": "2005.0",
                    "close": "2017.5",
                    "volume": "120"
                },
                {
                    "datetime": "2026-11-09 12:00:00",
                    "open": "2000.0",
                    "high": "2012.0",
                    "low": "1995.0",
                    "close": "2010.0",
                    "volume": "100"
                }
            ]
        });

        let open_1215 = Utc
            .with_ymd_and_hms(2026, 11, 9, 12, 15, 0)
            .single()
            .expect("valid datetime")
            .timestamp_millis();
        let (candles, live_price) = MarketDataFeed::parse_twelve_data_snapshot_with_now(
            &payload,
            Timeframe::M15,
            open_1215 + 5 * 60 * 1000,
            100,
        );

        assert_eq!(candles.len(), 1);
        assert_eq!(live_price, Some(2017.5));
    }

    #[test]
    fn test_broker_offset_applies_to_candles_and_live_price() {
        let mut snapshot = MarketFeedSnapshot {
            candles: vec![Candle {
                timestamp: 1,
                open: 2000.1,
                high: 2005.55,
                low: 1995.0,
                close: 2002.34,
                volume: 100.0,
                taker_buy_volume: 0.0,
                timeframe: Timeframe::M15,
            }],
            source: "TWELVEDATA".to_string(),
            live_price: Some(2003.33),
            volume_source: VOLUME_SOURCE_NONE.to_string(),
        };

        MarketDataFeed::apply_broker_offset(&mut snapshot, 2.37);

        assert!((snapshot.candles[0].open - 2002.47).abs() < 1e-9);
        assert!((snapshot.candles[0].high - 2007.92).abs() < 1e-9);
        assert!((snapshot.candles[0].low - 1997.37).abs() < 1e-9);
        assert!((snapshot.candles[0].close - 2004.71).abs() < 1e-9);
        assert_eq!(snapshot.live_price, Some(2005.7));
    }

    #[test]
    fn test_parse_spot_price_accepts_string_and_number() {
        assert_eq!(
            MarketDataFeed::parse_spot_price_json(&json!({"price": "4111.17"})),
            Some(4111.17)
        );
        assert_eq!(
            MarketDataFeed::parse_spot_price_json(&json!({"price": 4111.17})),
            Some(4111.17)
        );
    }

    #[test]
    fn test_parse_spot_price_rejects_invalid_payloads() {
        assert_eq!(
            MarketDataFeed::parse_spot_price_json(&json!({"price": "n/a"})),
            None
        );
        assert_eq!(
            MarketDataFeed::parse_spot_price_json(&json!({"price": 0})),
            None
        );
        assert_eq!(
            MarketDataFeed::parse_spot_price_json(&json!({"price": -1.0})),
            None
        );
        assert_eq!(
            MarketDataFeed::parse_spot_price_json(&json!({"message": "error"})),
            None
        );
    }

    #[test]
    fn test_shift_snapshot_prices_applies_basis_to_candles_and_live_price() {
        let mut snapshot = MarketFeedSnapshot {
            candles: vec![Candle {
                timestamp: 1,
                open: 2000.1,
                high: 2005.55,
                low: 1995.0,
                close: 2002.34,
                volume: 100.0,
                taker_buy_volume: 0.0,
                timeframe: Timeframe::M15,
            }],
            source: "BINANCE_PAXG".to_string(),
            live_price: Some(2003.33),
            volume_source: VOLUME_SOURCE_BINANCE_PAXG.to_string(),
        };

        MarketDataFeed::shift_snapshot_prices(&mut snapshot, 5.07);

        assert!((snapshot.candles[0].open - 2005.17).abs() < 1e-9);
        assert!((snapshot.candles[0].high - 2010.62).abs() < 1e-9);
        assert!((snapshot.candles[0].low - 2000.07).abs() < 1e-9);
        assert!((snapshot.candles[0].close - 2007.41).abs() < 1e-9);
        assert_eq!(snapshot.candles[0].volume, 100.0);
        assert!((snapshot.live_price.expect("live price") - 2008.4).abs() < 1e-9);
    }

    #[test]
    fn test_shift_snapshot_prices_ignores_zero_and_non_finite_basis() {
        let original = MarketFeedSnapshot {
            candles: vec![Candle {
                timestamp: 1,
                open: 2000.0,
                high: 2001.0,
                low: 1999.0,
                close: 2000.5,
                volume: 100.0,
                taker_buy_volume: 0.0,
                timeframe: Timeframe::M15,
            }],
            source: "BINANCE_PAXG".to_string(),
            live_price: Some(2000.5),
            volume_source: VOLUME_SOURCE_BINANCE_PAXG.to_string(),
        };

        for basis in [0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut snapshot = original.clone();
            MarketDataFeed::shift_snapshot_prices(&mut snapshot, basis);
            assert_eq!(snapshot.candles[0].open, original.candles[0].open);
            assert_eq!(snapshot.candles[0].close, original.candles[0].close);
            assert_eq!(snapshot.live_price, original.live_price);
        }
    }

    #[test]
    fn test_fallback_snapshot_uses_fallback_seed_source() {
        let snapshot = MarketFeedSnapshot::fallback(10, Timeframe::M15);

        assert_eq!(snapshot.source, "FALLBACK_SEED");
        assert_eq!(snapshot.candles.len(), 10);
        assert_eq!(snapshot.live_price, None);
        assert_eq!(snapshot.volume_source, VOLUME_SOURCE_NONE);
    }

    #[test]
    fn test_volume_source_labels_match_dashboard_contract() {
        assert_eq!(VOLUME_SOURCE_BINANCE_PAXG, "BINANCE_PAXG");
        assert_eq!(VOLUME_SOURCE_BINANCE_PAXG_PROXY, "BINANCE_PAXG_PROXY");
        assert_eq!(VOLUME_SOURCE_OANDA_TICK, "OANDA_TICK");
        assert_eq!(VOLUME_SOURCE_NONE, "NONE");
        assert_eq!(SOURCE_OANDA_XAUUSD, "OANDA_XAUUSD");
    }

    #[test]
    fn test_oanda_candles_parse_closed_volume_and_live_price() {
        let payload = json!({
            "candles": [
                {
                    "complete": true,
                    "volume": 1234,
                    "time": "2026-11-13T03:30:00.000000000Z",
                    "mid": {"o": "3900.0", "h": "3910.0", "l": "3895.0", "c": "3905.0"}
                },
                {
                    "complete": false,
                    "volume": 100,
                    "time": "2026-11-13T03:45:00.000000000Z",
                    "mid": {"o": "3905.0", "h": "3915.0", "l": "3900.0", "c": "3912.0"}
                }
            ]
        });
        let now_ms = DateTime::parse_from_rfc3339("2026-11-13T03:50:00Z")
            .expect("timestamp")
            .timestamp_millis();

        let (candles, live_price) = MarketDataFeed::parse_oanda_candles_with_now(
            &payload,
            Timeframe::M15,
            15 * 60 * 1000,
            now_ms,
            300,
        );

        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].volume, 1234.0);
        assert_eq!(candles[0].taker_buy_volume, 0.0);
        let expected_timestamp = DateTime::parse_from_rfc3339("2026-11-13T03:30:00Z")
            .expect("timestamp")
            .timestamp_millis();
        assert_eq!(candles[0].timestamp, expected_timestamp);
        assert_eq!(live_price, Some(3912.0));
    }

    fn test_ohlc_candle(
        timestamp: i64,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: f64,
    ) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::H1,
        }
    }

    #[test]
    fn test_parse_yahoo_chart_drops_unclosed_and_null_volume() {
        let now = Utc
            .with_ymd_and_hms(2026, 11, 16, 9, 0, 0)
            .single()
            .expect("timestamp");
        let payload = json!({
            "chart": {
                "result": [{
                    "timestamp": [1794816000_i64, 1794816900_i64, 1794819600_i64],
                    "indicators": {
                        "quote": [{
                            "open": [3860.0, 3861.0, 3862.0],
                            "high": [3865.0, 3866.0, 3867.0],
                            "low": [3855.0, 3856.0, 3857.0],
                            "close": [3863.0, 3864.0, 3865.0],
                            "volume": [1000.0, null, 3000.0]
                        }]
                    }
                }]
            }
        });

        let candles = MarketDataFeed::parse_yahoo_chart(
            &payload,
            Timeframe::M15,
            15 * 60 * 1000,
            now.timestamp_millis(),
        );

        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].timestamp, 1794816000_i64 * 1000);
        assert_eq!(candles[0].volume, 1000.0);
        assert_eq!(candles[0].taker_buy_volume, 0.0);
    }

    #[test]
    fn test_aggregate_h1_to_h4_sums_volume() {
        let candles = vec![
            test_ohlc_candle(1794787200 * 1000, 100.0, 102.0, 99.0, 101.0, 10.0),
            test_ohlc_candle(1794790800 * 1000, 101.0, 103.0, 100.0, 102.0, 20.0),
            test_ohlc_candle(1794794400 * 1000, 102.0, 104.0, 101.0, 103.0, 30.0),
            test_ohlc_candle(1794798000 * 1000, 103.0, 105.0, 102.0, 104.0, 40.0),
            test_ohlc_candle(1794801600 * 1000, 104.0, 106.0, 103.0, 105.0, 50.0),
        ];
        let now = Utc
            .with_ymd_and_hms(2026, 11, 16, 8, 0, 0)
            .single()
            .expect("timestamp");

        let h4 = MarketDataFeed::aggregate_candles(&candles, Timeframe::H4, now.timestamp_millis());

        assert_eq!(h4.len(), 2);
        assert_eq!(h4[0].timestamp, 1794787200 * 1000);
        assert_eq!(h4[0].open, 100.0);
        assert_eq!(h4[0].high, 105.0);
        assert_eq!(h4[0].low, 99.0);
        assert_eq!(h4[0].close, 104.0);
        assert_eq!(h4[0].volume, 100.0);
        assert_eq!(h4[1].timestamp, 1794801600 * 1000);
        assert_eq!(h4[1].volume, 50.0);
    }

    #[test]
    fn test_yahoo_daily_timestamp_maps_to_utc_midnight() {
        let yahoo_open = 1794546000_i64 * 1000; // 2026-11-13 05:00 UTC
        let midnight = 1794528000_i64 * 1000; // 2026-11-13 00:00 UTC
        assert_eq!(yahoo_daily_timestamp(yahoo_open), midnight);
    }

    #[test]
    fn test_recent_volume_matches_ignores_weekend_history() {
        let price: Vec<Candle> = (0..100)
            .map(|index| test_candle(index * 900_000, 0.0, 0.0))
            .collect();
        let volume: Vec<Candle> = (80..100)
            .map(|index| test_candle(index * 900_000, 10.0, 0.0))
            .collect();

        let (matches, total) = MarketDataFeed::recent_volume_matches(&price, &volume, 20);
        assert_eq!((matches, total), (20, 20));
        // Overall coverage is only 20%, which is why the M15 acceptance test
        // uses the recent window instead.
        assert_eq!(MarketDataFeed::count_volume_matches(&price, &volume), 20);
    }

    #[test]
    fn test_merge_yahoo_volume_sets_zero_taker_buy() {
        let mut price = vec![test_candle(1794816000 * 1000, 0.0, 999.0)];
        let yahoo = vec![test_candle(1794816000 * 1000, 12345.0, 0.0)];

        assert_eq!(MarketDataFeed::count_volume_matches(&price, &yahoo), 1);
        assert!(MarketDataFeed::merge_volume_proxy(&mut price, &yahoo));
        assert_eq!(price[0].volume, 12345.0);
        assert_eq!(price[0].taker_buy_volume, 0.0);
    }

    #[test]
    fn test_volume_source_labels_include_yahoo_futures_proxy() {
        assert_eq!(VOLUME_SOURCE_YAHOO_FUTURES_PROXY, "YAHOO_FUTURES_PROXY");
        assert_eq!(
            SOURCE_TWELVEDATA_YAHOO_VOLUME,
            "TWELVEDATA_XAUUSD+YAHOO_FUTURES_VOLUME"
        );
        assert!(volume_source_has_buy_sell_delta(
            VOLUME_SOURCE_BINANCE_PAXG_PROXY
        ));
        assert!(!volume_source_has_buy_sell_delta(
            VOLUME_SOURCE_YAHOO_FUTURES_PROXY
        ));
        assert!(!volume_source_has_buy_sell_delta(VOLUME_SOURCE_OANDA_TICK));
        assert!(!volume_source_has_buy_sell_delta(VOLUME_SOURCE_EXNESS_TICK));
        assert!(!volume_source_has_buy_sell_delta(VOLUME_SOURCE_NONE));
    }

    #[test]
    fn test_yahoo_interval_and_lookback() {
        assert_eq!(yahoo_interval(Timeframe::M15), "15m");
        assert_eq!(yahoo_interval(Timeframe::H1), "1h");
        assert_eq!(yahoo_interval(Timeframe::H4), "1h");
        assert!(yahoo_lookback_days(Timeframe::M15, 300) >= 4);
        assert!(yahoo_lookback_days(Timeframe::H1, 300) >= 12);
        assert!(yahoo_lookback_days(Timeframe::H4, 300) >= 50);
    }

    #[tokio::test]
    #[ignore = "network: hits Yahoo GC=F"]
    async fn test_yahoo_fetch_live_returns_volume() {
        let feed = MarketDataFeed::new().with_yahoo_futures_volume_enabled(true);
        let m15 = feed
            .fetch_yahoo_volume(Timeframe::M15, 300)
            .await
            .expect("Yahoo M15 volume");
        assert!(!m15.is_empty());
        assert!(m15.iter().any(|candle| candle.volume > 0.0));

        let h4 = feed
            .fetch_yahoo_volume(Timeframe::H4, 300)
            .await
            .expect("Yahoo H4 volume");
        assert!(!h4.is_empty());
        assert!(h4.iter().any(|candle| candle.volume > 0.0));

        let d1 = feed
            .fetch_yahoo_daily_volume(300)
            .await
            .expect("Yahoo D1 volume");
        assert!(!d1.is_empty());
        assert!(d1.iter().any(|candle| candle.volume > 0.0));
        assert!(d1
            .iter()
            .all(|candle| candle.timestamp.rem_euclid(DAY_MS) == 0));
    }

    #[test]
    fn test_binance_interval_maps_supported_timeframes() {
        assert_eq!(binance_interval(Timeframe::M15), "15m");
        assert_eq!(binance_interval(Timeframe::H1), "1h");
        assert_eq!(binance_interval(Timeframe::H4), "4h");
    }
}
