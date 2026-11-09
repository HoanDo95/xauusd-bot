use crate::models::{BotConfig, SignalType};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Mức độ tác động của tin tức kinh tế tới giá Vàng (XAU/USD)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImpactLevel {
    High,   // Đỏ: NFP, CPI, FOMC, GDP
    Medium, // Cam: PMI, Doanh số bán lẻ, Jobless Claims
    Low,    // Vàng/Xanh: Tin phụ ít biến động
}

/// Xu hướng vĩ mô định hướng thị trường (Macro Bias)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacroBias {
    #[serde(rename = "BULLISH")]
    Bullish, // Vĩ mô ủng hộ Vàng tăng (DXY suy yếu, lạm phát/cắt giảm lãi suất)
    #[serde(rename = "BEARISH")]
    Bearish, // Vĩ mô ủng hộ Vàng giảm (DXY tăng vọt, FED thắt chặt tiền tệ)
    #[serde(rename = "NEUTRAL")]
    Neutral, // Thị trường cân bằng, cho phép cả lệnh Mua lẫn Bán
}

impl MacroBias {
    /// Chuỗi cấu hình chuẩn dùng cho BotConfig.macro_bias
    pub fn as_str(&self) -> &'static str {
        match self {
            MacroBias::Bullish => "BULLISH",
            MacroBias::Bearish => "BEARISH",
            MacroBias::Neutral => "NEUTRAL",
        }
    }

    /// Parse không phân biệt hoa/thường, mặc định NEUTRAL với dữ liệu lạ.
    pub fn from_str_ci(value: &str) -> Self {
        match value.trim().to_uppercase().as_str() {
            "BULLISH" => MacroBias::Bullish,
            "BEARISH" => MacroBias::Bearish,
            _ => MacroBias::Neutral,
        }
    }
}

impl std::fmt::Display for MacroBias {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MacroBias::Bullish => write!(f, "BULLISH (Ưu tiên Mua)"),
            MacroBias::Bearish => write!(f, "BEARISH (Ưu tiên Bán)"),
            MacroBias::Neutral => write!(f, "NEUTRAL (Cân bằng)"),
        }
    }
}

/// Cấu trúc một sự kiện tin tức kinh tế (Economic Event)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomicEvent {
    pub id: String,
    pub title: String,
    pub currency: String, // "USD"
    pub impact: ImpactLevel,
    pub scheduled_time: DateTime<Utc>,
    pub forecast: Option<String>,
    pub previous: Option<String>,
    pub actual: Option<String>,
}

/// Cấu hình Bộ lọc Tin tức & Kill Switch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsFilterConfig {
    /// Bật/Tắt cơ chế tự động dừng giao dịch khi có tin đỏ (Kill Switch)
    pub kill_switch_enabled: bool,
    /// Số phút tạm dừng TRƯỚC khi tin lớn công bố (Mặc định: 30 phút)
    pub pre_news_buffer_minutes: i64,
    /// Số phút tạm dừng SAU khi tin lớn công bố (Mặc định: 30 phút để giá ổn định)
    pub post_news_buffer_minutes: i64,
    /// Định hướng vĩ mô hiện tại
    pub macro_bias: MacroBias,
    /// Chỉ lọc tin có mức độ High Impact
    pub block_high_impact_only: bool,
}

impl Default for NewsFilterConfig {
    fn default() -> Self {
        Self {
            kill_switch_enabled: true,
            pre_news_buffer_minutes: 30,
            post_news_buffer_minutes: 30,
            macro_bias: MacroBias::Bullish,
            block_high_impact_only: true,
        }
    }
}

/// Dịch vụ Kiểm tra & Bảo vệ Tài khoản trước Tin tức (Macro & News Kill Switch)
#[derive(Debug, Clone)]
pub struct NewsFilterService {
    pub config: NewsFilterConfig,
    pub calendar_events: Vec<EconomicEvent>,
}

/// Kiểm tra tính an toàn của thị trường trước tin tức (Standalone evaluator).
/// Trả về (is_safe, blocked_event, explanation).
pub fn is_market_safe_from_news(
    current_time: DateTime<Utc>,
    events: &[EconomicEvent],
    pre_buffer_minutes: i64,
    post_buffer_minutes: i64,
) -> (bool, Option<EconomicEvent>, String) {
    let pre_buffer = pre_buffer_minutes.max(0);
    let post_buffer = post_buffer_minutes.max(0);

    for event in events {
        // Chỉ xét tin USD có tác động mạnh (High Impact: NFP, CPI, FOMC, GDP)
        if event.impact != ImpactLevel::High || !event.currency.eq_ignore_ascii_case("USD") {
            continue;
        }

        let start_blackout = event.scheduled_time - Duration::minutes(pre_buffer);
        let end_blackout = event.scheduled_time + Duration::minutes(post_buffer);

        if current_time >= start_blackout && current_time <= end_blackout {
            let minutes_diff = (event.scheduled_time - current_time).num_minutes();
            let reason = if minutes_diff > 0 {
                format!(
                    "🔴 KILL SWITCH KÍCH HOẠT: Sắp ra tin lớn [{}] sau {} phút. Tạm ngưng bắn tín hiệu!",
                    event.title, minutes_diff
                )
            } else {
                format!(
                    "🔴 KILL SWITCH KÍCH HOẠT: Tin lớn [{}] vừa ra {} phút trước. Đang chờ thị trường triệt tiêu sóng giật!",
                    event.title,
                    minutes_diff.abs()
                )
            };

            return (false, Some(event.clone()), reason);
        }
    }

    (
        true,
        None,
        "🟢 THỊ TRƯỜNG AN TOÀN: Không có sự kiện kinh tế đỏ trong khung giờ cấm".to_string(),
    )
}

impl NewsFilterService {
    /// Khởi tạo service với lịch tin rỗng; lịch thật được nạp qua `update_events`.
    pub fn new(config: NewsFilterConfig) -> Self {
        Self {
            config,
            calendar_events: Vec::new(),
        }
    }

    /// Đồng bộ lịch tin tức mới (ForexFactory JSON hoặc fallback).
    pub fn update_events(&mut self, events: Vec<EconomicEvent>) {
        self.calendar_events = events;
        self.calendar_events
            .sort_by_key(|event| event.scheduled_time);
    }

    /// Trả về `true` nếu thị trường AN TOÀN, `false` nếu bị chặn bởi tin High Impact.
    pub fn check_kill_switch(&self, now: DateTime<Utc>, bot_config: &BotConfig) -> bool {
        if !bot_config.kill_switch_enabled {
            return true;
        }
        self.get_current_blocking_event(now, bot_config).is_none()
    }

    /// Sự kiện High-Impact USD đang nằm trong cửa sổ cấm (nếu có).
    pub fn get_current_blocking_event(
        &self,
        now: DateTime<Utc>,
        bot_config: &BotConfig,
    ) -> Option<&EconomicEvent> {
        if !bot_config.kill_switch_enabled {
            return None;
        }

        let pre_buffer = bot_config.pre_news_buffer_minutes.max(0);
        let post_buffer = bot_config.post_news_buffer_minutes.max(0);

        self.calendar_events.iter().find(|event| {
            event.impact == ImpactLevel::High
                && event.currency.eq_ignore_ascii_case("USD")
                && now >= event.scheduled_time - Duration::minutes(pre_buffer)
                && now <= event.scheduled_time + Duration::minutes(post_buffer)
        })
    }

    /// Trả về (is_kill_switch_active, active_event, reason) cho Dashboard & Worker.
    pub fn evaluate_news_status(
        &self,
        now: DateTime<Utc>,
        bot_config: &BotConfig,
    ) -> (bool, Option<EconomicEvent>, String) {
        if !bot_config.kill_switch_enabled {
            return (
                false,
                None,
                "Kill Switch đang TẮT (Chế độ giao dịch tự do)".to_string(),
            );
        }

        let (is_safe, event, msg) = is_market_safe_from_news(
            now,
            &self.calendar_events,
            bot_config.pre_news_buffer_minutes,
            bot_config.post_news_buffer_minutes,
        );

        (!is_safe, event, msg)
    }

    /// Lọc tín hiệu theo Hướng Vĩ Mô (Macro Bias Filter).
    /// Chỉ chấp thuận tín hiệu thuận xu hướng lớn để tăng xác suất thắng.
    pub fn validate_macro_bias(&self, signal_type: SignalType) -> (bool, String) {
        match (self.config.macro_bias, signal_type) {
            (MacroBias::Neutral, _) => (true, "Định hướng vĩ mô Trung lập (Neutral), chấp thuận cả lệnh Mua và Bán".to_string()),
            (MacroBias::Bullish, SignalType::Buy) => (true, "Tín hiệu MUA thuận xu hướng vĩ mô BULLISH (+20% xác suất thắng)".to_string()),
            (MacroBias::Bullish, SignalType::Sell) => (false, "Lọc bỏ tín hiệu BÁN do đi ngược định hướng vĩ mô dài hạn BULLISH (Tránh bắt đỉnh khi trend tăng)".to_string()),
            (MacroBias::Bearish, SignalType::Sell) => (true, "Tín hiệu BÁN thuận xu hướng vĩ mô BEARISH (+20% xác suất thắng)".to_string()),
            (MacroBias::Bearish, SignalType::Buy) => (false, "Lọc bỏ tín hiệu MUA do đi ngược định hướng vĩ mô dài hạn BEARISH (Tránh bắt đáy khi trend giảm)".to_string()),
        }
    }

    /// Tạo dữ liệu lịch kinh tế mẫu chân thực xung quanh thời điểm hiện tại
    /// (dùng cho fallback khi không có mạng hoặc môi trường test).
    pub fn generate_mock_economic_calendar() -> Vec<EconomicEvent> {
        let now = Utc::now();
        vec![
            EconomicEvent {
                id: "EVT-CPI-01".to_string(),
                title: "US Core CPI m/m (Chỉ số giá tiêu dùng cốt lõi)".to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::High,
                scheduled_time: now + Duration::hours(2),
                forecast: Some("0.3%".to_string()),
                previous: Some("0.3%".to_string()),
                actual: None,
            },
            EconomicEvent {
                id: "EVT-NFP-02".to_string(),
                title: "US Non-Farm Payrolls (Bảng lương phi nông nghiệp)".to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::High,
                scheduled_time: now + Duration::days(1) + Duration::hours(12),
                forecast: Some("175K".to_string()),
                previous: Some("216K".to_string()),
                actual: None,
            },
            EconomicEvent {
                id: "EVT-FOMC-03".to_string(),
                title: "FOMC Rate Decision & Press Conference (Quyết định lãi suất FED)"
                    .to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::High,
                scheduled_time: now + Duration::days(3),
                forecast: Some("5.25%".to_string()),
                previous: Some("5.50%".to_string()),
                actual: None,
            },
            EconomicEvent {
                id: "EVT-PMI-04".to_string(),
                title: "US ISM Manufacturing PMI".to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::Medium,
                scheduled_time: now - Duration::hours(4),
                forecast: Some("49.2".to_string()),
                previous: Some("48.5".to_string()),
                actual: Some("49.8".to_string()),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn high_impact_event(scheduled_time: DateTime<Utc>) -> EconomicEvent {
        EconomicEvent {
            id: "TEST-CPI".to_string(),
            title: "US CPI m/m".to_string(),
            currency: "USD".to_string(),
            impact: ImpactLevel::High,
            scheduled_time,
            forecast: Some("0.3%".to_string()),
            previous: Some("0.2%".to_string()),
            actual: None,
        }
    }

    fn bot_config() -> BotConfig {
        BotConfig {
            kill_switch_enabled: true,
            pre_news_buffer_minutes: 30,
            post_news_buffer_minutes: 30,
            ..BotConfig::default()
        }
    }

    #[test]
    fn test_kill_switch_blocks_within_30_minutes_window() {
        let event_time = Utc
            .with_ymd_and_hms(2026, 11, 9, 13, 30, 0)
            .single()
            .expect("valid UTC timestamp");
        let mut service = NewsFilterService::new(NewsFilterConfig::default());
        service.update_events(vec![high_impact_event(event_time)]);
        let config = bot_config();

        // 30 phút trước tin -> chặn
        assert!(!service.check_kill_switch(event_time - Duration::minutes(30), &config));
        // Đúng giờ ra tin -> chặn
        assert!(!service.check_kill_switch(event_time, &config));
        // 30 phút sau tin -> chặn
        assert!(!service.check_kill_switch(event_time + Duration::minutes(30), &config));
    }

    #[test]
    fn test_kill_switch_allows_outside_30_minutes_window() {
        let event_time = Utc
            .with_ymd_and_hms(2026, 11, 9, 13, 30, 0)
            .single()
            .expect("valid UTC timestamp");
        let mut service = NewsFilterService::new(NewsFilterConfig::default());
        service.update_events(vec![high_impact_event(event_time)]);
        let config = bot_config();

        assert!(service.check_kill_switch(event_time - Duration::minutes(31), &config));
        assert!(service.check_kill_switch(event_time + Duration::minutes(31), &config));
    }

    #[test]
    fn test_kill_switch_disabled_always_allows_market() {
        let event_time = Utc
            .with_ymd_and_hms(2026, 11, 9, 13, 30, 0)
            .single()
            .expect("valid UTC timestamp");
        let mut service = NewsFilterService::new(NewsFilterConfig::default());
        service.update_events(vec![high_impact_event(event_time)]);
        let config = BotConfig {
            kill_switch_enabled: false,
            ..BotConfig::default()
        };

        assert!(service.check_kill_switch(event_time, &config));
        assert!(service
            .get_current_blocking_event(event_time, &config)
            .is_none());
    }

    #[test]
    fn test_medium_impact_does_not_trigger_kill_switch() {
        let event_time = Utc
            .with_ymd_and_hms(2026, 11, 9, 13, 30, 0)
            .single()
            .expect("valid UTC timestamp");
        let mut service = NewsFilterService::new(NewsFilterConfig::default());
        let mut event = high_impact_event(event_time);
        event.impact = ImpactLevel::Medium;
        service.update_events(vec![event]);

        assert!(service.check_kill_switch(event_time, &bot_config()));
    }

    #[test]
    fn test_evaluate_news_status_reports_active_event() {
        let event_time = Utc
            .with_ymd_and_hms(2026, 11, 9, 13, 30, 0)
            .single()
            .expect("valid UTC timestamp");
        let mut service = NewsFilterService::new(NewsFilterConfig::default());
        service.update_events(vec![high_impact_event(event_time)]);

        let (active, event, reason) =
            service.evaluate_news_status(event_time - Duration::minutes(10), &bot_config());
        assert!(active);
        assert_eq!(event.map(|e| e.id), Some("TEST-CPI".to_string()));
        assert!(reason.contains("KILL SWITCH"));
    }
}
