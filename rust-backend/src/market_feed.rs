use crate::models::{Candle, Timeframe};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tracing::{info, warn};

/// Ảnh chụp dữ liệu thị trường tại một thời điểm: nến đã đóng + giá live + nguồn.
#[derive(Debug, Clone)]
pub struct MarketFeedSnapshot {
    /// Chỉ chứa các cây nến đã đóng cửa hoàn tất.
    pub candles: Vec<Candle>,
    /// "TWELVEDATA" | "BINANCE_PAXG" | "FALLBACK_SEED".
    pub source: String,
    /// Giá live từ cây nến đang chạy (nếu có).
    pub live_price: Option<f64>,
}

impl MarketFeedSnapshot {
    fn fallback(limit: usize, timeframe: Timeframe) -> Self {
        Self {
            candles: MarketDataFeed::generate_fallback_gold_candles(limit, timeframe),
            source: "FALLBACK_SEED".to_string(),
            live_price: None,
        }
    }
}

/// Nguồn dữ liệu OHLCV Vàng miễn phí 24/7.
/// - Ưu tiên TwelveData `XAU/USD` nếu có `TWELVEDATA_API_KEY`.
/// - Mặc định dùng Binance Public API `PAXGUSDT` (không cần API key).
/// - Offline: fallback về bộ sinh nến mẫu an toàn.
#[derive(Debug, Clone)]
pub struct MarketDataFeed {
    client: Client,
    twelve_data_api_key: Option<String>,
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

        Self {
            client,
            twelve_data_api_key,
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
                warn!("⚠️ {} spot request lỗi: {}", provider, err);
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
                warn!("⚠️ {} spot JSON lỗi: {}", provider, err);
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
        let preferred = if self.twelve_data_api_key.is_some() {
            "TWELVEDATA"
        } else {
            "BINANCE_PAXG"
        };
        self.fetch_market_snapshot(timeframe, limit, preferred, 0.0)
            .await
            .candles
    }

    /// Nến HTF đã đóng cửa cho bối cảnh D1/H4/H1 — chỉ Binance PAXGUSDT.
    /// Không fallback seed: lỗi mạng trả về rỗng để caller giữ context cũ.
    pub async fn fetch_htf_candles(&self, timeframe: Timeframe, limit: usize) -> Vec<Candle> {
        let interval = binance_interval(timeframe);
        self.fetch_binance_closed_candles(interval, timeframe, limit)
            .await
            .unwrap_or_default()
    }

    /// Nến ngày (1d) từ Binance PAXGUSDT cho bối cảnh D1.
    ///
    /// `models::Timeframe` chưa có biến thể D1 (để không đụng tới backtest), nên nến
    /// được gắn tag H4 — chỉ dùng nội bộ để tính context, không phát tín hiệu.
    pub async fn fetch_daily_htf_candles(&self, limit: usize) -> Vec<Candle> {
        self.fetch_binance_closed_candles("1d", Timeframe::H4, limit)
            .await
            .unwrap_or_default()
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
            .map_err(|e| format!("Binance HTF request error: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Binance HTF HTTP {}", response.status()));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("Binance HTF JSON parse error: {}", e))?;

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
        let prefer_twelve = preferred_source.trim().eq_ignore_ascii_case("TWELVEDATA");
        let has_twelve = self.twelve_data_api_key.is_some();

        let mut snapshot = if prefer_twelve && has_twelve {
            let twelve = self.fetch_twelve_snapshot(timeframe, limit).await;
            if twelve.is_some() {
                twelve
            } else {
                warn!("⚠️ TwelveData không khả dụng, chuyển sang Binance PAXGUSDT.");
                self.fetch_binance_snapshot(timeframe, limit).await
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
        match self.fetch_twelve_data(timeframe, limit, api_key).await {
            Ok(snapshot) if !snapshot.candles.is_empty() => Some(snapshot),
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
            .map_err(|e| format!("Binance request error: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Binance HTTP {}", response.status()));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("Binance JSON parse error: {}", e))?;

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
        })
    }

    async fn fetch_twelve_data(
        &self,
        timeframe: Timeframe,
        limit: usize,
        api_key: &str,
    ) -> Result<MarketFeedSnapshot, String> {
        let interval = match timeframe {
            Timeframe::M15 => "15min",
            Timeframe::H1 => "1h",
            Timeframe::H4 => "4h",
        };
        let url = format!(
            "https://api.twelvedata.com/time_series?symbol=XAU/USD&interval={}&outputsize={}&apikey={}",
            interval,
            limit + 1,
            api_key
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("TwelveData request error: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("TwelveData HTTP {}", response.status()));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| format!("TwelveData JSON parse error: {}", e))?;

        if let Some(message) = payload.get("message").and_then(|v| v.as_str()) {
            return Err(format!("TwelveData API error: {}", message));
        }

        let (candles, live_price) = Self::parse_twelve_data_snapshot_with_now(
            &payload,
            timeframe,
            Utc::now().timestamp_millis(),
            limit,
        );

        info!(
            "📡 [MarketFeed] TwelveData XAU/USD {}: {} nến đã đóng cửa, live={:?}.",
            timeframe,
            candles.len(),
            live_price
        );
        Ok(MarketFeedSnapshot {
            candles,
            source: "TWELVEDATA".to_string(),
            live_price,
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
    pub(crate) fn parse_twelve_data_snapshot_with_now(
        payload: &Value,
        timeframe: Timeframe,
        now_ms: i64,
        limit: usize,
    ) -> (Vec<Candle>, Option<f64>) {
        let Some(rows) = payload.get("values").and_then(|v| v.as_array()) else {
            return (Vec::new(), None);
        };

        let step_ms = timeframe_millis(timeframe);
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
                value_to_f64(row.get("volume").unwrap_or(&Value::Null)),
            );
            let (Some(open), Some(high), Some(low), Some(close), Some(volume)) = parsed else {
                continue;
            };

            if !is_finite_ohlcv(open, high, low, close, volume) {
                continue;
            }

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
        .ok()?;
    Some(Utc.from_utc_datetime(&naive).timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    }

    #[test]
    fn test_binance_interval_maps_supported_timeframes() {
        assert_eq!(binance_interval(Timeframe::M15), "15m");
        assert_eq!(binance_interval(Timeframe::H1), "1h");
        assert_eq!(binance_interval(Timeframe::H4), "4h");
    }
}
