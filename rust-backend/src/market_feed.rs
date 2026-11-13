use crate::models::{Candle, Timeframe};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Nhãn nguồn volume ổn định cho dashboard/health.
pub const VOLUME_SOURCE_BINANCE_PAXG: &str = "BINANCE_PAXG";
pub const VOLUME_SOURCE_BINANCE_PAXG_PROXY: &str = "BINANCE_PAXG_PROXY";
pub const VOLUME_SOURCE_OANDA_TICK: &str = "OANDA_TICK";
pub const VOLUME_SOURCE_NONE: &str = "NONE";
pub const SOURCE_OANDA_XAUUSD: &str = "OANDA_XAUUSD";

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

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

/// Nguồn dữ liệu OHLCV Vàng miễn phí 24/7.
/// - `data_source=TWELVEDATA`: OHLC từ TwelveData `XAU/USD` + volume proxy PAXG.
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

        Self {
            client,
            twelve_data_api_key,
            twelve_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            oanda_api_token,
            oanda_base_url,
            oanda_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
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
            let (twelve, binance) = tokio::join!(
                self.fetch_twelve_snapshot(timeframe, limit),
                self.fetch_binance_closed_candles(binance_interval(timeframe), timeframe, limit),
            );
            let binance = binance.ok();
            if let Some(mut snapshot) = twelve {
                let price_source =
                    Self::twelve_htf_price_source(&mut snapshot.candles, binance.as_deref());
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
            let (twelve, binance) = tokio::join!(
                self.fetch_twelve_daily_snapshot(limit),
                self.fetch_binance_closed_candles("1d", Timeframe::H4, limit),
            );
            let binance = binance.ok();
            if let Some(mut snapshot) = twelve {
                let price_source =
                    Self::twelve_htf_price_source(&mut snapshot.candles, binance.as_deref());
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
        volume_candles: Option<&[Candle]>,
    ) -> String {
        let matched = volume_candles
            .map(|volume_candles| Self::merge_volume_proxy(candles, volume_candles))
            .unwrap_or(false);
        if matched {
            "TWELVEDATA_XAUUSD+PAXG_VOLUME".to_string()
        } else {
            "TWELVEDATA_XAUUSD".to_string()
        }
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
            // TwelveData chỉ có OHLC; lấy Binance song song để làm volume proxy.
            let (twelve, binance) = tokio::join!(
                self.fetch_twelve_snapshot(timeframe, limit),
                self.fetch_binance_snapshot(timeframe, limit),
            );
            match twelve {
                Some(mut twelve) => {
                    let matched = binance
                        .as_ref()
                        .map(|binance| {
                            Self::merge_volume_proxy(&mut twelve.candles, &binance.candles)
                        })
                        .unwrap_or(false);
                    twelve.volume_source = if matched {
                        VOLUME_SOURCE_BINANCE_PAXG_PROXY.to_string()
                    } else {
                        VOLUME_SOURCE_NONE.to_string()
                    };
                    if matched {
                        info!(
                            "🔊 [MarketFeed] Đã ghép volume Binance PAXG vào {} nến TwelveData.",
                            twelve.candles.len()
                        );
                    } else {
                        warn!("⚠️ TwelveData không khớp timestamp với Binance PAXG — volume = 0.");
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
        "M15" => Duration::from_secs(300),
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
        assert_eq!(twelve_cache_ttl("M15"), Duration::from_secs(300));
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

    #[test]
    fn test_binance_interval_maps_supported_timeframes() {
        assert_eq!(binance_interval(Timeframe::M15), "15m");
        assert_eq!(binance_interval(Timeframe::H1), "1h");
        assert_eq!(binance_interval(Timeframe::H4), "4h");
    }
}
