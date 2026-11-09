use crate::models::{Candle, Timeframe};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use tracing::{info, warn};

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

    /// Kéo nến VÀ CHỈ trả về các cây nến đã đóng cửa hoàn tất (anti-repaint).
    pub async fn fetch_closed_gold_candles(
        &self,
        timeframe: Timeframe,
        limit: usize,
    ) -> Vec<Candle> {
        let limit = limit.max(5);

        if let Some(api_key) = &self.twelve_data_api_key {
            match self.fetch_twelve_data(timeframe, limit, api_key).await {
                Ok(candles) if !candles.is_empty() => return candles,
                Ok(_) => warn!("⚠️ TwelveData trả về rỗng, chuyển sang Binance PAXGUSDT."),
                Err(err) => warn!("⚠️ TwelveData lỗi ({}), chuyển sang Binance PAXGUSDT.", err),
            }
        }

        match self.fetch_binance(timeframe, limit).await {
            Ok(candles) if !candles.is_empty() => candles,
            Ok(_) => {
                warn!("⚠️ Binance trả về rỗng, dùng nến fallback offline.");
                Self::generate_fallback_gold_candles(limit, timeframe)
            }
            Err(err) => {
                warn!(
                    "⚠️ Không kết nối được Binance ({}), dùng nến fallback offline.",
                    err
                );
                Self::generate_fallback_gold_candles(limit, timeframe)
            }
        }
    }

    async fn fetch_binance(
        &self,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<Vec<Candle>, String> {
        let interval = match timeframe {
            Timeframe::M15 => "15m",
            Timeframe::H1 => "1h",
            Timeframe::H4 => "4h",
        };
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

        let candles =
            Self::parse_binance_klines_with_now(&payload, timeframe, Utc::now().timestamp_millis());

        info!(
            "📡 [MarketFeed] Binance PAXGUSDT {}: {} nến đã đóng cửa.",
            timeframe,
            candles.len()
        );
        Ok(candles)
    }

    async fn fetch_twelve_data(
        &self,
        timeframe: Timeframe,
        limit: usize,
        api_key: &str,
    ) -> Result<Vec<Candle>, String> {
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

        let candles = Self::parse_twelve_data_values_with_now(
            &payload,
            timeframe,
            Utc::now().timestamp_millis(),
            limit,
        );

        info!(
            "📡 [MarketFeed] TwelveData XAU/USD {}: {} nến đã đóng cửa.",
            timeframe,
            candles.len()
        );
        Ok(candles)
    }

    /// Parse mảng klines Binance và loại bỏ cây nến chưa đóng cửa.
    pub(crate) fn parse_binance_klines_with_now(
        payload: &Value,
        timeframe: Timeframe,
        now_ms: i64,
    ) -> Vec<Candle> {
        let Some(rows) = payload.as_array() else {
            return Vec::new();
        };

        let mut candles = Vec::with_capacity(rows.len());
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

            // Bỏ cây nến đang chạy: close_time nằm trong tương lai.
            if close_time > now_ms {
                continue;
            }

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

            candles.push(Candle {
                timestamp: open_time,
                open,
                high,
                low,
                close,
                volume,
                timeframe,
            });
        }

        candles
    }

    /// Parse TwelveData time_series values (mảng mới nhất đứng đầu).
    pub(crate) fn parse_twelve_data_values_with_now(
        payload: &Value,
        timeframe: Timeframe,
        now_ms: i64,
        limit: usize,
    ) -> Vec<Candle> {
        let Some(rows) = payload.get("values").and_then(|v| v.as_array()) else {
            return Vec::new();
        };

        let step_ms = timeframe_millis(timeframe);
        let mut candles = Vec::with_capacity(rows.len());

        for row in rows {
            let Some(datetime) = row.get("datetime").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(timestamp) = parse_twelve_data_datetime(datetime) else {
                continue;
            };

            // Cây nến chưa đóng cửa: thời điểm đóng = open + step > now.
            if timestamp + step_ms > now_ms {
                continue;
            }

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

            candles.push(Candle {
                timestamp,
                open,
                high,
                low,
                close,
                volume,
                timeframe,
            });
        }

        candles.sort_by_key(|candle| candle.timestamp);
        if candles.len() > limit {
            let keep_from = candles.len() - limit;
            candles.drain(0..keep_from);
        }
        candles
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
                "0",
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
}
