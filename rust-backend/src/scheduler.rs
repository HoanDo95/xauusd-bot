use crate::analyzer::GoldMarketAnalyzer;
use crate::models::{Candle, DailyMacroReport};
use crate::news_filter::{EconomicEvent, ImpactLevel, MacroBias};
use chrono::{DateTime, Local, Utc};
use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tracing::{info, warn};

const FOREX_FACTORY_CALENDAR_URL: &str = "https://nfs.faireconomy.media/ff_calendar_thisweek.json";
const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/openai";
const OPENROUTER_API_BASE: &str = "https://openrouter.ai/api/v1";
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
const DEFAULT_OPENROUTER_MODEL: &str = "anthropic/claude-3.5-sonnet";

/// Nhà cung cấp AI tùy chọn cho báo cáo vĩ mô ngày.
///
/// Tín hiệu giao dịch cốt lõi KHÔNG phụ thuộc AI — luôn có fallback EMA50/200 miễn phí.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiProvider {
    None,
    Gemini,
    OpenRouter,
}

impl AiProvider {
    /// Xác định provider từ `AI_PROVIDER` và các key hiện có.
    ///
    /// Hàm thuần (pure) để test được mà không cần sửa process env:
    /// - `AI_PROVIDER` hợp lệ (`NONE`/`GEMINI`/`OPENROUTER`, không phân biệt hoa thường)
    ///   được ưu tiên tuyệt đối.
    /// - Rỗng hoặc không hợp lệ → auto-detect: Gemini trước, rồi OpenRouter.
    pub fn from_env_values(ai_provider: &str, gemini_key: &str, openrouter_key: &str) -> Self {
        match ai_provider.trim().to_ascii_uppercase().as_str() {
            "NONE" => Self::None,
            "GEMINI" => Self::Gemini,
            "OPENROUTER" => Self::OpenRouter,
            _ => {
                if !gemini_key.trim().is_empty() {
                    Self::Gemini
                } else if !openrouter_key.trim().is_empty() {
                    Self::OpenRouter
                } else {
                    Self::None
                }
            }
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gemini => "gemini",
            Self::OpenRouter => "openrouter",
        }
    }
}

impl std::fmt::Display for AiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Cấu hình AI đã resolve (không bao giờ log key).
struct AiSettings {
    provider: AiProvider,
    api_key: String,
    api_base: String,
    model: String,
}

fn resolve_model(value: &str, default: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Module lập lịch định kỳ 07:00 AM mỗi ngày (Daily Cron Job & Macro Connector)
pub struct DailyScheduler {
    http_client: Client,
    provider: AiProvider,
    ai_api_key: String,
    ai_api_base: String, // OpenRouter, Gemini OpenAI-compatible, hoặc endpoint tương thích
    ai_model: String,
}

impl DailyScheduler {
    /// Constructor tương thích ngược cho tests/callers cũ.
    ///
    /// Có key + không có base → OpenRouter (đúng hành vi cũ); không có key → `None`.
    pub fn new(ai_api_key: String, ai_api_base: Option<String>) -> Self {
        let provider = if ai_api_key.trim().is_empty() {
            AiProvider::None
        } else {
            AiProvider::OpenRouter
        };
        Self {
            http_client: Self::build_http_client(),
            provider,
            ai_api_key,
            ai_api_base: ai_api_base.unwrap_or_else(|| OPENROUTER_API_BASE.to_string()),
            ai_model: DEFAULT_OPENROUTER_MODEL.to_string(),
        }
    }

    /// Khởi tạo từ biến môi trường với auto-detect provider.
    ///
    /// Chỉ log provider/model — tuyệt đối không log API key.
    pub fn from_env() -> Self {
        let read = |name: &str| std::env::var(name).unwrap_or_default();
        let settings = Self::resolve_ai_settings(
            &read("AI_PROVIDER"),
            &read("GEMINI_API_KEY"),
            &read("OPENROUTER_API_KEY"),
            &read("GEMINI_MODEL"),
            &read("OPENROUTER_MODEL"),
        );

        let scheduler = Self {
            http_client: Self::build_http_client(),
            provider: settings.provider,
            ai_api_key: settings.api_key,
            ai_api_base: settings.api_base,
            ai_model: settings.model,
        };
        scheduler.log_ai_configuration();
        scheduler
    }

    fn build_http_client() -> Client {
        Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new())
    }

    /// Hàm thuần resolve provider + key + base + model (test không cần env/network).
    fn resolve_ai_settings(
        ai_provider: &str,
        gemini_key: &str,
        openrouter_key: &str,
        gemini_model: &str,
        openrouter_model: &str,
    ) -> AiSettings {
        let provider = AiProvider::from_env_values(ai_provider, gemini_key, openrouter_key);
        let (api_key, api_base, model) = match provider {
            AiProvider::Gemini => (
                gemini_key.trim().to_string(),
                GEMINI_API_BASE.to_string(),
                resolve_model(gemini_model, DEFAULT_GEMINI_MODEL),
            ),
            AiProvider::OpenRouter => (
                openrouter_key.trim().to_string(),
                OPENROUTER_API_BASE.to_string(),
                resolve_model(openrouter_model, DEFAULT_OPENROUTER_MODEL),
            ),
            AiProvider::None => (String::new(), String::new(), String::new()),
        };

        AiSettings {
            provider,
            api_key,
            api_base,
            model,
        }
    }

    fn has_ai_credentials(&self) -> bool {
        self.provider != AiProvider::None
            && !self.ai_api_key.trim().is_empty()
            && self.ai_api_key != "MOCK_KEY"
    }

    fn log_ai_configuration(&self) {
        match self.provider {
            AiProvider::None => {
                info!("🤖 AI provider: none — báo cáo vĩ mô dùng EMA50/200 miễn phí.");
            }
            _ if !self.has_ai_credentials() => {
                warn!(
                    "⚠️ AI provider '{}' được chọn nhưng thiếu API key — dùng báo cáo EMA50/200.",
                    self.provider.as_str()
                );
            }
            _ => {
                info!(
                    "🤖 AI provider: {} | model: {} | chỉ dùng cho báo cáo vĩ mô ngày.",
                    self.provider.as_str(),
                    self.ai_model
                );
            }
        }
    }

    /// Nhãn nguồn báo cáo để log mà không lộ key.
    pub fn report_source(&self) -> &'static str {
        match self.provider {
            AiProvider::Gemini if self.has_ai_credentials() => "Gemini AI",
            AiProvider::OpenRouter if self.has_ai_credentials() => "OpenRouter AI",
            _ => "EMA50/200 (fallback)",
        }
    }

    /// 1. Đồng bộ lịch kinh tế tuần này từ ForexFactory Public JSON.
    ///
    /// Lọc USD + High/Medium impact; fallback an toàn khi offline.
    pub async fn fetch_daily_economic_events(&self) -> Vec<EconomicEvent> {
        info!("📅 [Daily Cron 07:00 AM] Đang đồng bộ lịch tin ForexFactory...");

        match self
            .http_client
            .get(FOREX_FACTORY_CALENDAR_URL)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                match response.json::<serde_json::Value>().await {
                    Ok(payload) => {
                        let events = Self::parse_forexfactory_events(&payload);
                        if events.is_empty() {
                            warn!(
                                "⚠️ ForexFactory không có sự kiện USD High/Medium, dùng lịch fallback."
                            );
                            Self::fallback_economic_events()
                        } else {
                            info!(
                                "✅ Đã đồng bộ {} sự kiện USD từ ForexFactory.",
                                events.len()
                            );
                            events
                        }
                    }
                    Err(err) => {
                        warn!(
                            "⚠️ Không parse được ForexFactory JSON ({}), dùng lịch fallback.",
                            err
                        );
                        Self::fallback_economic_events()
                    }
                }
            }
            Ok(response) => {
                warn!(
                    "⚠️ ForexFactory HTTP {}, dùng lịch fallback.",
                    response.status()
                );
                Self::fallback_economic_events()
            }
            Err(err) => {
                warn!(
                    "⚠️ Không kết nối được ForexFactory ({}), dùng lịch fallback.",
                    err
                );
                Self::fallback_economic_events()
            }
        }
    }

    /// Parse mảng JSON ForexFactory thành EconomicEvent (chỉ USD, High/Medium).
    pub fn parse_forexfactory_events(payload: &serde_json::Value) -> Vec<EconomicEvent> {
        let Some(rows) = payload.as_array() else {
            return Vec::new();
        };

        let mut events = Vec::new();

        for (index, row) in rows.iter().enumerate() {
            let country = row.get("country").and_then(|v| v.as_str()).unwrap_or("");
            if !country.eq_ignore_ascii_case("USD") {
                continue;
            }

            let Some(impact) = row.get("impact").and_then(|v| v.as_str()) else {
                continue;
            };
            let impact = match impact.trim().to_lowercase().as_str() {
                "high" => ImpactLevel::High,
                "medium" => ImpactLevel::Medium,
                _ => continue,
            };

            let Some(date_text) = row.get("date").and_then(|v| v.as_str()) else {
                continue;
            };
            let Ok(scheduled_time) = DateTime::parse_from_rfc3339(date_text) else {
                continue;
            };
            let scheduled_time = scheduled_time.with_timezone(&Utc);

            let title = row
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("US Economic Event")
                .to_string();

            events.push(EconomicEvent {
                id: format!("FF-{}-{}", scheduled_time.format("%Y%m%dT%H%M"), index),
                title,
                currency: "USD".to_string(),
                impact,
                scheduled_time,
                forecast: optional_string(row, "forecast"),
                previous: optional_string(row, "previous"),
                actual: optional_string(row, "actual"),
            });
        }

        events.sort_by_key(|event| event.scheduled_time);
        events
    }

    /// Lịch tin mẫu chân thực dùng khi offline (luôn có tin High Impact).
    pub fn fallback_economic_events() -> Vec<EconomicEvent> {
        let now = Utc::now();
        vec![
            EconomicEvent {
                id: format!("CAL-{}-CPI", now.format("%Y%m%d")),
                title: "US Core CPI m/m (Chỉ số Lạm phát lõi Mỹ)".to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::High,
                scheduled_time: now + chrono::Duration::hours(12),
                forecast: Some("0.3%".to_string()),
                previous: Some("0.3%".to_string()),
                actual: None,
            },
            EconomicEvent {
                id: format!("CAL-{}-NFP", now.format("%Y%m%d")),
                title: "US Non-Farm Employment Change (Bảng lương phi nông nghiệp)".to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::High,
                scheduled_time: now + chrono::Duration::hours(36),
                forecast: Some("175K".to_string()),
                previous: Some("216K".to_string()),
                actual: None,
            },
            EconomicEvent {
                id: format!("CAL-{}-FOMC", now.format("%Y%m%d")),
                title: "FOMC Rate Decision (Quyết định Lãi suất FED)".to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::High,
                scheduled_time: now + chrono::Duration::hours(60),
                forecast: Some("5.25%".to_string()),
                previous: Some("5.50%".to_string()),
                actual: None,
            },
            EconomicEvent {
                id: format!("CAL-{}-PMI", now.format("%Y%m%d")),
                title: "US ISM Services PMI".to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::Medium,
                scheduled_time: now + chrono::Duration::hours(14),
                forecast: Some("52.4".to_string()),
                previous: Some("51.8".to_string()),
                actual: None,
            },
        ]
    }

    /// 2. Báo cáo vĩ mô MIỄN PHÍ dựa trên EMA50/EMA200 + biên độ ATR của nến thật.
    pub fn build_quant_macro_report(candles: &[Candle]) -> DailyMacroReport {
        let now = Utc::now();
        let bias = GoldMarketAnalyzer::detect_ema_trend_bias(candles);
        let current_price = candles.last().map(|c| c.close).unwrap_or(2684.50);
        let ema50 = GoldMarketAnalyzer::calculate_ema(candles, 50);
        let ema200 = GoldMarketAnalyzer::calculate_ema(candles, 200);

        let recent_count = candles.len().clamp(1, 20);
        let average_range = candles
            .iter()
            .rev()
            .take(recent_count)
            .map(|c| c.range())
            .sum::<f64>()
            / (recent_count as f64);
        let forecast_range = average_range.max(2.0) * 2.0;
        let low = current_price - forecast_range;
        let high = current_price + forecast_range;

        let (confidence, rationale, action) = match bias {
            MacroBias::Bullish => (
                86u8,
                format!(
                    "Giá đóng cửa {:.2} nằm trên EMA50 {:.2} và EMA50 đang dốc lên; cấu trúc xu hướng tăng được xác nhận bằng dữ liệu nến thật (EMA200 tham chiếu {:.2}).",
                    current_price, ema50, ema200
                ),
                "ƯU TIÊN BUY THUẬN XU HƯỚNG: chỉ canh mua tại Hỗ trợ động, bỏ qua tín hiệu Bán rủi ro.".to_string(),
            ),
            MacroBias::Bearish => (
                86u8,
                format!(
                    "Giá đóng cửa {:.2} nằm dưới EMA50 {:.2} và EMA50 đang dốc xuống; áp lực bán chủ đạo (EMA200 tham chiếu {:.2}).",
                    current_price, ema50, ema200
                ),
                "ƯU TIÊN SELL THUẬN XU HƯỚNG: chỉ canh bán tại Kháng cự động, bỏ qua tín hiệu Mua rủi ro.".to_string(),
            ),
            MacroBias::Neutral => (
                65u8,
                format!(
                    "Giá {:.2} đang giằng co quanh EMA50 {:.2}; chưa có độ dốc xu hướng rõ ràng, ưu tiên đứng ngoài chờ hội tụ.",
                    current_price, ema50
                ),
                "NEUTRAL: chỉ giao dịch khi có đủ Pinbar + Volume tại vùng cản mạnh.".to_string(),
            ),
        };

        DailyMacroReport {
            date: now.format("%Y-%m-%d").to_string(),
            macro_bias: bias,
            confidence_score: confidence,
            gold_range_forecast: format!("${:.2} - ${:.2}", low, high),
            daily_rationale: rationale,
            high_impact_events_today: Vec::new(),
            recommended_action: action,
            updated_at: now,
        }
    }

    /// 3. Báo cáo vĩ mô dự phòng khi chỉ có giá hiện tại (không có chuỗi nến).
    pub fn heuristic_price_report(current_price: f64) -> DailyMacroReport {
        let now = Utc::now();
        DailyMacroReport {
            date: now.format("%Y-%m-%d").to_string(),
            macro_bias: MacroBias::Bullish,
            confidence_score: 85,
            gold_range_forecast: format!(
                "${:.2} - ${:.2}",
                current_price - 22.0,
                current_price + 22.0
            ),
            daily_rationale: "Chưa đủ chuỗi nến để tính EMA50/EMA200; báo cáo tạm dùng định hướng trung tính nghiêng tăng. Worker sẽ tự cập nhật khi có nến thật.".to_string(),
            high_impact_events_today: Vec::new(),
            recommended_action: "Chờ nến thật đóng cửa để xác nhận xu hướng EMA trước khi vào lệnh.".to_string(),
            updated_at: now,
        }
    }

    /// Gọi AI Macro (tùy chọn, chỉ khi user cấu hình key). Luôn fallback an toàn.
    pub async fn fetch_daily_macro_bias(&self, current_price: f64) -> DailyMacroReport {
        let fallback = Self::heuristic_price_report(current_price);
        let prompt = Self::build_macro_prompt(current_price, &fallback);
        self.request_ai_report(&prompt, fallback).await
    }

    /// Báo cáo vĩ mô deterministic từ nến thật + tùy chọn AI nếu có key.
    pub async fn fetch_daily_macro_bias_from_candles(
        &self,
        candles: &[Candle],
    ) -> DailyMacroReport {
        let fallback = Self::build_quant_macro_report(candles);
        if candles.is_empty() {
            return fallback;
        }
        let prompt = Self::build_macro_prompt(
            candles.last().map(|c| c.close).unwrap_or(2684.50),
            &fallback,
        );
        self.request_ai_report(&prompt, fallback).await
    }

    fn build_macro_prompt(current_price: f64, fallback: &DailyMacroReport) -> String {
        format!(
            "Bạn là Giám đốc Phân tích Định lượng (Chief Macro Strategist) cho quỹ giao dịch Vàng XAU/USD. \
            Hôm nay là {}. Giá vàng hiện tại là ${:.2}. Xu hướng EMA định lượng hiện tại: {} với độ tin cậy {}%. \
            Bối cảnh: {}. \
            Hãy phân tích xu hướng DXY, lãi suất Fed và lợi suất trái phiếu Mỹ 10Y. \
            Trả về duy nhất định dạng JSON chuẩn: \
            {{\"macro_bias\": \"Bullish\" | \"Bearish\" | \"Neutral\", \"confidence_score\": 85, \
            \"gold_range_forecast\": \"$2670 - $2715\", \
            \"daily_rationale\": \"Giải thích ngắn gọn 2 câu về dòng tiền vĩ mô\", \
            \"recommended_action\": \"Ưu tiên Buy khi test hỗ trợ\"}}",
            Utc::now().format("%Y-%m-%d"),
            current_price,
            fallback.macro_bias.as_str(),
            fallback.confidence_score,
            fallback.daily_rationale
        )
    }

    async fn request_ai_report(
        &self,
        prompt: &str,
        fallback: DailyMacroReport,
    ) -> DailyMacroReport {
        if !self.has_ai_credentials() {
            return fallback;
        }

        let url = format!(
            "{}/chat/completions",
            self.ai_api_base.trim_end_matches('/')
        );
        let payload = json!({
            "model": self.ai_model,
            "messages": [
                { "role": "system", "content": "You are an institutional macro quantitative analyst. Always output pure valid JSON without markdown wrapping." },
                { "role": "user", "content": prompt }
            ],
            "response_format": { "type": "json_object" },
            "temperature": 0.2
        });

        let response = match self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.ai_api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                warn!(
                    "Không thể kết nối AI provider '{}' ({}). Dùng báo cáo định lượng EMA.",
                    self.provider.as_str(),
                    err
                );
                return fallback;
            }
        };

        if !response.status().is_success() {
            warn!(
                "AI provider '{}' trả HTTP {}. Dùng báo cáo định lượng EMA.",
                self.provider.as_str(),
                response.status()
            );
            return fallback;
        }

        let body = match response.json::<serde_json::Value>().await {
            Ok(body) => body,
            Err(err) => {
                warn!(
                    "AI provider '{}' trả JSON lỗi ({}). Dùng báo cáo định lượng EMA.",
                    self.provider.as_str(),
                    err
                );
                return fallback;
            }
        };

        let Some(content) = body["choices"][0]["message"]["content"].as_str() else {
            warn!(
                "AI provider '{}' thiếu choices[0].message.content. Dùng báo cáo định lượng EMA.",
                self.provider.as_str()
            );
            return fallback;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) else {
            warn!(
                "AI provider '{}' trả nội dung không phải JSON hợp lệ. Dùng báo cáo định lượng EMA.",
                self.provider.as_str()
            );
            return fallback;
        };

        let bias = MacroBias::from_str_ci(
            parsed["macro_bias"]
                .as_str()
                .unwrap_or(fallback.macro_bias.as_str()),
        );
        let now = Utc::now();

        DailyMacroReport {
            date: now.format("%Y-%m-%d").to_string(),
            macro_bias: bias,
            confidence_score: parsed["confidence_score"]
                .as_u64()
                .unwrap_or(fallback.confidence_score as u64)
                .min(100) as u8,
            gold_range_forecast: parsed["gold_range_forecast"]
                .as_str()
                .unwrap_or(&fallback.gold_range_forecast)
                .to_string(),
            daily_rationale: parsed["daily_rationale"]
                .as_str()
                .unwrap_or(&fallback.daily_rationale)
                .to_string(),
            high_impact_events_today: fallback.high_impact_events_today,
            recommended_action: parsed["recommended_action"]
                .as_str()
                .unwrap_or(&fallback.recommended_action)
                .to_string(),
            updated_at: now,
        }
    }

    /// 4. Tính toán thời gian ngủ cho tới 07:00:00 sáng hôm sau (local time).
    pub fn seconds_until_next_seven_am() -> u64 {
        let now = Local::now();
        let target_today = now.date_naive().and_hms_opt(7, 0, 0).unwrap_or_default();

        let target_datetime = if now.time() < target_today.time() {
            target_today
                .and_local_timezone(Local)
                .earliest()
                .unwrap_or(now)
        } else {
            let tomorrow = now.date_naive() + chrono::Duration::days(1);
            tomorrow
                .and_hms_opt(7, 0, 0)
                .unwrap_or_default()
                .and_local_timezone(Local)
                .earliest()
                .unwrap_or(now)
        };

        target_datetime
            .signed_duration_since(now)
            .num_seconds()
            .max(1) as u64
    }
}

fn optional_string(row: &serde_json::Value, key: &str) -> Option<String> {
    row.get(key)
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use serde_json::json;

    #[test]
    fn test_parse_forexfactory_filters_usd_high_medium() {
        let payload = json!([
            {
                "title": "US Core CPI m/m",
                "country": "USD",
                "date": "2026-11-09T13:30:00-05:00",
                "impact": "High",
                "forecast": "0.3%",
                "previous": "0.2%"
            },
            {
                "title": "ECB President Lagarde Speaks",
                "country": "EUR",
                "date": "2026-11-09T14:00:00+01:00",
                "impact": "High"
            },
            {
                "title": "US ISM Services PMI",
                "country": "USD",
                "date": "2026-11-09T10:00:00-05:00",
                "impact": "Medium",
                "forecast": "52.4"
            },
            {
                "title": "US Low Impact Speech",
                "country": "USD",
                "date": "2026-11-09T11:00:00-05:00",
                "impact": "Low"
            }
        ]);

        let events = DailyScheduler::parse_forexfactory_events(&payload);
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.currency == "USD"));
        assert!(events.iter().any(|event| event.impact == ImpactLevel::High));
        assert!(events
            .iter()
            .any(|event| event.impact == ImpactLevel::Medium));
        // Sự kiện phải được sắp xếp tăng dần theo giờ.
        assert!(events[0].scheduled_time <= events[1].scheduled_time);
    }

    #[test]
    fn test_fallback_economic_events_contain_usd_high_impact() {
        let events = DailyScheduler::fallback_economic_events();
        assert!(!events.is_empty());
        assert!(events.iter().all(|event| event.currency == "USD"));
        assert!(events.iter().any(|event| event.impact == ImpactLevel::High));
    }

    #[tokio::test]
    async fn test_daily_macro_bias_generation() {
        let scheduler = DailyScheduler::new(String::new(), None);
        let report = scheduler.fetch_daily_macro_bias(2684.50).await;
        assert_eq!(report.macro_bias, MacroBias::Bullish);
        assert!(report.confidence_score >= 80);
    }

    #[test]
    fn test_quant_macro_report_detects_bearish_ema_trend() {
        let candles: Vec<Candle> = (0..80)
            .map(|i| {
                let close = 2700.0 - i as f64;
                Candle {
                    timestamp: 1_700_000_000_000 + i * 900_000,
                    open: close,
                    high: close + 1.0,
                    low: close - 1.0,
                    close,
                    volume: 1000.0,
                    timeframe: Timeframe::M15,
                }
            })
            .collect();

        let report = DailyScheduler::build_quant_macro_report(&candles);
        assert_eq!(report.macro_bias, MacroBias::Bearish);
        assert!(report.daily_rationale.contains("EMA50"));
        assert!(!report.gold_range_forecast.is_empty());
    }

    #[test]
    fn ai_provider_auto_detects_from_keys() {
        assert_eq!(
            AiProvider::from_env_values("", "gemini-key", ""),
            AiProvider::Gemini
        );
        assert_eq!(
            AiProvider::from_env_values("", "", "openrouter-key"),
            AiProvider::OpenRouter
        );
        // Cả hai key: Gemini được ưu tiên theo yêu cầu.
        assert_eq!(
            AiProvider::from_env_values("", "gemini-key", "openrouter-key"),
            AiProvider::Gemini
        );
        assert_eq!(AiProvider::from_env_values("", "", ""), AiProvider::None);
    }

    #[test]
    fn ai_provider_explicit_env_overrides_auto_detect() {
        assert_eq!(
            AiProvider::from_env_values("OPENROUTER", "gemini-key", "openrouter-key"),
            AiProvider::OpenRouter
        );
        assert_eq!(
            AiProvider::from_env_values("  openrouter  ", "gemini-key", "openrouter-key"),
            AiProvider::OpenRouter
        );
        assert_eq!(
            AiProvider::from_env_values("Gemini", "", "openrouter-key"),
            AiProvider::Gemini
        );
        assert_eq!(
            AiProvider::from_env_values("none", "gemini-key", "openrouter-key"),
            AiProvider::None
        );
    }

    #[test]
    fn resolve_ai_settings_for_gemini_uses_openai_compatible_base() {
        let defaults = DailyScheduler::resolve_ai_settings("", "gemini-key", "", "", "");
        assert_eq!(defaults.provider, AiProvider::Gemini);
        assert_eq!(defaults.api_key, "gemini-key");
        assert_eq!(defaults.api_base, GEMINI_API_BASE);
        assert_eq!(defaults.model, DEFAULT_GEMINI_MODEL);

        let custom =
            DailyScheduler::resolve_ai_settings("GEMINI", " gemini-key ", "", "gemini-2.5-pro", "");
        assert_eq!(custom.provider, AiProvider::Gemini);
        assert_eq!(custom.api_key, "gemini-key");
        assert_eq!(custom.model, "gemini-2.5-pro");
    }

    #[test]
    fn resolve_ai_settings_for_openrouter_keeps_legacy_defaults() {
        let defaults = DailyScheduler::resolve_ai_settings("", "", "openrouter-key", "", "");
        assert_eq!(defaults.provider, AiProvider::OpenRouter);
        assert_eq!(defaults.api_key, "openrouter-key");
        assert_eq!(defaults.api_base, OPENROUTER_API_BASE);
        assert_eq!(defaults.model, "anthropic/claude-3.5-sonnet");

        let custom = DailyScheduler::resolve_ai_settings(
            "openrouter",
            "",
            "openrouter-key",
            "",
            "openai/gpt-4o-mini",
        );
        assert_eq!(custom.provider, AiProvider::OpenRouter);
        assert_eq!(custom.model, "openai/gpt-4o-mini");
    }

    #[test]
    fn resolve_ai_settings_for_none_has_no_key_or_endpoint() {
        let settings = DailyScheduler::resolve_ai_settings("", "", "", "", "");
        assert_eq!(settings.provider, AiProvider::None);
        assert!(settings.api_key.is_empty());
        assert!(settings.api_base.is_empty());
        assert!(settings.model.is_empty());
    }

    #[test]
    fn new_constructor_backward_compatible() {
        let with_key = DailyScheduler::new("legacy-key".to_string(), None);
        assert_eq!(with_key.provider, AiProvider::OpenRouter);
        assert_eq!(with_key.ai_api_base, OPENROUTER_API_BASE);
        assert_eq!(with_key.ai_model, DEFAULT_OPENROUTER_MODEL);
        assert_eq!(with_key.report_source(), "OpenRouter AI");

        let without_key = DailyScheduler::new(String::new(), None);
        assert_eq!(without_key.provider, AiProvider::None);
        assert_eq!(without_key.report_source(), "EMA50/200 (fallback)");

        let custom_base = DailyScheduler::new(
            "legacy-key".to_string(),
            Some("http://localhost:1234/v1".into()),
        );
        assert_eq!(custom_base.ai_api_base, "http://localhost:1234/v1");
    }
}
