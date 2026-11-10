use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Re-export news domain types so every module shares one canonical import path.
pub use crate::news_filter::{EconomicEvent, ImpactLevel, MacroBias};

/// Khung thời gian hỗ trợ phân tích
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Timeframe {
    #[serde(rename = "M15")]
    M15,
    #[serde(rename = "H1")]
    H1,
    #[serde(rename = "H4")]
    H4,
}

impl std::fmt::Display for Timeframe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Timeframe::M15 => write!(f, "M15"),
            Timeframe::H1 => write!(f, "H1"),
            Timeframe::H4 => write!(f, "H4"),
        }
    }
}

/// Cấu trúc dữ liệu nến chuẩn OHLCV
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    /// Binance kline field 9: taker buy base asset volume. TwelveData/fallback
    /// candles do not expose this field, so it defaults to 0.0 for compatibility
    /// with older cached state and third-party feeds.
    #[serde(default)]
    pub taker_buy_volume: f64,
    pub timeframe: Timeframe,
}

impl Candle {
    /// Tính tổng biên độ nến (High - Low)
    pub fn range(&self) -> f64 {
        (self.high - self.low).max(0.0001)
    }

    /// Tính độ lớn thân nến (|Close - Open|)
    pub fn body_size(&self) -> f64 {
        (self.close - self.open).abs()
    }

    /// Tính râu trên (Upper Wick)
    pub fn upper_wick(&self) -> f64 {
        self.high - self.open.max(self.close)
    }

    /// Tính râu dưới (Lower Wick)
    pub fn lower_wick(&self) -> f64 {
        self.open.min(self.close) - self.low
    }

    /// Kiểm tra nến tăng hay giảm
    pub fn is_bullish(&self) -> bool {
        self.close >= self.open
    }
}

/// Loại tín hiệu giao dịch
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalType {
    #[serde(rename = "BUY")]
    Buy,
    #[serde(rename = "SELL")]
    Sell,
}

impl std::fmt::Display for SignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignalType::Buy => write!(f, "BUY / LONG"),
            SignalType::Sell => write!(f, "SELL / SHORT"),
        }
    }
}

/// Cách vào lệnh: khớp thị trường ngay, chờ retest giới hạn, hoặc breakout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum EntryType {
    #[default]
    Market,
    Limit,
    Breakout,
}

impl EntryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryType::Market => "MARKET",
            EntryType::Limit => "LIMIT",
            EntryType::Breakout => "BREAKOUT",
        }
    }
}

/// Trạng thái của tín hiệu
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalStatus {
    Active,
    Target1Hit,
    Target2Hit,
    StopLossHit,
    Cancelled,
}

/// Vùng Hỗ trợ / Kháng cự động được xác định từ các đỉnh/đáy sóng (Pivots)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportResistanceZone {
    pub level_type: ZoneType,
    pub price: f64,
    pub min_price: f64,
    pub max_price: f64,
    pub touch_count: usize,
    pub timeframe: Timeframe,
    pub strength_score: f64, // 0.0 -> 1.0 dựa trên số lần retest & volume
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneType {
    Support,
    Resistance,
}

/// Thực thể Tín hiệu hoàn chỉnh sinh ra từ thuật toán
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: String,
    pub symbol: String, // e.g. "XAU/USD"
    pub signal_type: SignalType,
    pub timeframe: Timeframe,
    pub entry_price: f64,
    pub stop_loss: f64,
    pub take_profit_1: f64,
    pub take_profit_2: f64,
    pub risk_points: f64,
    pub reward_points_tp1: f64,
    pub reward_points_tp2: f64,
    pub risk_reward_ratio: f64,
    pub volume_surge_ratio: f64,
    /// Khối lượng vào lệnh tự động (Standard Lot)
    pub recommended_lot_size: f64,
    /// Số tiền rủi ro tối đa ($)
    pub risk_amount_usd: f64,
    /// Lợi nhuận dự kiến tại TP1 ($)
    pub potential_profit_tp1_usd: f64,
    /// Lợi nhuận dự kiến tại TP2 ($)
    pub potential_profit_tp2_usd: f64,
    pub price_action_notes: String,
    pub volume_analysis_notes: String,
    pub full_rationale: String,
    pub status: SignalStatus,
    pub created_at: DateTime<Utc>,
    pub telegram_message_id: Option<i64>,
    /// Khóa chống bắn trùng: "{symbol}_{timeframe}_{candle_timestamp}"
    #[serde(default)]
    pub dedup_key: String,
    /// Timestamp (ms) của cây nến đã đóng cửa sinh ra tín hiệu
    #[serde(default)]
    pub candle_timestamp: i64,
    /// Lệnh mô phỏng (bấm test) không tiêu hao quota 1 lệnh/ngày
    #[serde(default)]
    pub is_simulated: bool,
    /// Điểm hội tụ kỹ thuật 0..=100
    #[serde(default)]
    pub confluence_score: u8,
    /// Thời điểm chốt trạng thái TP1/TP2/SL (nếu có)
    #[serde(default)]
    pub outcome_at: Option<DateTime<Utc>>,
    /// Cách vào lệnh: MARKET / LIMIT / BREAKOUT.
    #[serde(default)]
    pub entry_type: EntryType,
    /// Giá đóng cửa của cây nến Pinbar đã sinh tín hiệu.
    #[serde(default)]
    pub candle_close_price: f64,
    /// Giá live tại thời điểm phát tín hiệu (nếu có).
    #[serde(default)]
    pub live_market_price: Option<f64>,
    /// Giá Limit Retest đề xuất (nếu có).
    #[serde(default)]
    pub retest_limit_price: Option<f64>,
    /// Nguồn dữ liệu giá dùng để sinh tín hiệu.
    #[serde(default)]
    pub data_source: String,
    /// Lệnh Limit/Breakout đã được khớp giá hay chưa.
    #[serde(default = "default_entry_filled")]
    pub is_entry_filled: bool,
}

fn default_entry_filled() -> bool {
    // Tín hiệu cũ (trước Phase A) đều là MARKET và đã vào ngay.
    true
}

/// Cấu hình tham số bot và quản lý rủi ro
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    pub symbol: String,
    pub timeframe: Timeframe,
    /// Tỷ lệ R:R tối thiểu để kích hoạt lệnh (Mặc định: 2.0 tức 1:2)
    pub min_risk_reward: f64,
    /// Khoảng đệm Stop Loss an toàn tính bằng giá Vàng (Mặc định: 8.0 - 10.0$)
    pub gold_sl_buffer_points: f64,
    /// Hệ số Volume đột biến so với đường trung bình MA (Mặc định: 1.5x)
    pub volume_surge_multiplier: f64,
    /// Tỷ lệ tối thiểu giữa râu nến từ chối và thân nến (Mặc định: 2.0x)
    pub rejection_wick_ratio: f64,
    /// Số nến nhìn lại để tìm đỉnh đáy hỗ trợ/kháng cự (Lookback)
    pub lookback_period: usize,
    /// Chu kỳ trung bình cộng khối lượng (Volume MA Period)
    pub volume_ma_period: usize,
    /// Tổng số dư tài khoản giao dịch (Account Balance USD, e.g. $10,000)
    pub account_balance: f64,
    /// Rủi ro trên mỗi lệnh (% vốn tài khoản, Mặc định: 1.0%)
    pub risk_percentage: f64,
    /// Đòn bẩy tài khoản (Account Leverage, e.g. 1:100)
    pub account_leverage: f64,
    /// Telegram Bot Token
    pub telegram_bot_token: String,
    /// Telegram Chat ID
    pub telegram_chat_id: String,
    /// Trạng thái hoạt động
    pub is_active: bool,
    /// Bật/Tắt cơ chế dừng giao dịch khi có tin vĩ mô lớn (News Kill Switch)
    pub kill_switch_enabled: bool,
    /// Thời gian chặn trước khi ra tin lớn (phút)
    pub pre_news_buffer_minutes: i64,
    /// Thời gian chờ thị trường ổn định sau tin lớn (phút)
    pub post_news_buffer_minutes: i64,
    /// Định hướng xu hướng vĩ mô dài hạn: "BULLISH", "BEARISH", "NEUTRAL" hoặc "AUTO" (EMA50)
    pub macro_bias: String,
    /// Kích hoạt bộ lọc quét Khối Lệnh Tổ Chức (Order Block Filter)
    pub order_block_filter: bool,
    /// Số tín hiệu thật tối đa được phát mỗi ngày (mặc định 1 — Sniper Mode)
    #[serde(default = "default_max_signals_per_day")]
    pub max_signals_per_day: usize,
    /// Bật bộ lọc khung giờ thanh khoản Âu-Mỹ 13:00-23:00 GMT+7
    #[serde(default = "default_session_filter_enabled")]
    pub session_filter_enabled: bool,
    /// Nguồn dữ liệu giá đang sử dụng: "BINANCE_PAXG" hoặc "TWELVEDATA"
    #[serde(default = "default_data_source")]
    pub data_source: String,
    /// Mật khẩu bảo vệ webhook TradingView (rỗng = không bắt buộc)
    #[serde(default)]
    pub webhook_passphrase: String,
    /// Cách vào lệnh: "MARKET", "LIMIT_RETEST" hoặc "BREAKOUT".
    #[serde(default = "default_entry_mode")]
    pub entry_mode: String,
    /// Chênh lệch giá broker so với nguồn dữ liệu (USD), cộng vào mọi giá.
    #[serde(default)]
    pub broker_price_offset: f64,
    /// Tự động căn giá PAXG (Binance) về giá spot XAU/USD tham chiếu.
    /// TwelveData/gold-api chỉ cung cấp giá spot, không thay thế OHLCV + volume.
    #[serde(default = "default_true")]
    pub use_auto_spot_basis: bool,
    /// Sai lệch tối đa giữa giá live và giá đóng nến để vào MARKET (USD).
    #[serde(default = "default_max_entry_drift_points")]
    pub max_entry_drift_points: f64,
    /// Tuổi tối đa của cây nến đã đóng trước khi từ chối tín hiệu (giây).
    #[serde(default = "default_max_candle_age_secs")]
    pub max_candle_age_secs: i64,
    /// Bật/Tắt briefing Telegram trước phiên Âu/Mỹ (không tiêu hao quota).
    #[serde(default = "default_session_alerts_enabled")]
    pub session_alerts_enabled: bool,
    /// Số phút báo trước khi phiên mở (15..=120, mặc định 60).
    #[serde(default = "default_session_alert_lead_minutes")]
    pub session_alert_lead_minutes: i64,
}

fn default_max_signals_per_day() -> usize {
    1
}

fn default_entry_mode() -> String {
    "MARKET".to_string()
}

fn default_max_entry_drift_points() -> f64 {
    2.5
}

fn default_max_candle_age_secs() -> i64 {
    120
}

fn default_session_filter_enabled() -> bool {
    true
}

fn default_data_source() -> String {
    "BINANCE_PAXG".to_string()
}

fn default_session_alerts_enabled() -> bool {
    true
}

fn default_true() -> bool {
    true
}

fn default_session_alert_lead_minutes() -> i64 {
    60
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            symbol: "XAU/USD".to_string(),
            timeframe: Timeframe::M15,
            min_risk_reward: 2.0,
            gold_sl_buffer_points: 8.5,
            volume_surge_multiplier: 1.5,
            rejection_wick_ratio: 2.0,
            lookback_period: 30,
            volume_ma_period: 20,
            account_balance: 10000.0,
            risk_percentage: 1.0,
            account_leverage: 100.0,
            telegram_bot_token: "".to_string(),
            telegram_chat_id: "".to_string(),
            is_active: true,
            kill_switch_enabled: true,
            pre_news_buffer_minutes: 30,
            post_news_buffer_minutes: 30,
            macro_bias: "AUTO".to_string(),
            order_block_filter: true,
            max_signals_per_day: default_max_signals_per_day(),
            session_filter_enabled: default_session_filter_enabled(),
            data_source: default_data_source(),
            webhook_passphrase: String::new(),
            entry_mode: default_entry_mode(),
            broker_price_offset: 0.0,
            use_auto_spot_basis: true,
            max_entry_drift_points: default_max_entry_drift_points(),
            max_candle_age_secs: default_max_candle_age_secs(),
            session_alerts_enabled: default_session_alerts_enabled(),
            session_alert_lead_minutes: default_session_alert_lead_minutes(),
        }
    }
}

impl BotConfig {
    /// Chuẩn hóa cấu hình từ Dashboard/API: chặn giá trị NaN, âm hoặc vượt biên.
    /// Trả về cấu hình an toàn thay vì để engine chạy với tham số nguy hiểm.
    pub fn sanitized(mut self) -> Self {
        fn finite_or(value: f64, fallback: f64) -> f64 {
            if value.is_finite() {
                value
            } else {
                fallback
            }
        }

        self.symbol = self.symbol.trim().to_string();
        if self.symbol.is_empty() {
            self.symbol = "XAU/USD".to_string();
        }

        self.min_risk_reward = finite_or(self.min_risk_reward, 2.0).clamp(1.5, 5.0);
        self.gold_sl_buffer_points = finite_or(self.gold_sl_buffer_points, 8.5).clamp(5.0, 25.0);
        self.volume_surge_multiplier = finite_or(self.volume_surge_multiplier, 1.5).clamp(1.1, 3.5);
        self.rejection_wick_ratio = finite_or(self.rejection_wick_ratio, 2.0).clamp(1.5, 4.0);
        self.lookback_period = self.lookback_period.clamp(15, 100);
        self.volume_ma_period = self.volume_ma_period.clamp(5, 50);
        self.account_balance = finite_or(self.account_balance, 10_000.0).max(100.0);
        self.risk_percentage = finite_or(self.risk_percentage, 1.0).clamp(0.1, 5.0);
        self.account_leverage = finite_or(self.account_leverage, 100.0).clamp(1.0, 1000.0);
        self.max_signals_per_day = self.max_signals_per_day.clamp(1, 5);
        self.pre_news_buffer_minutes = self.pre_news_buffer_minutes.clamp(0, 120);
        self.post_news_buffer_minutes = self.post_news_buffer_minutes.clamp(0, 120);

        self.macro_bias = match self.macro_bias.trim().to_uppercase().as_str() {
            "BULLISH" => "BULLISH",
            "BEARISH" => "BEARISH",
            "NEUTRAL" => "NEUTRAL",
            _ => "AUTO",
        }
        .to_string();

        self.data_source = match self.data_source.trim().to_uppercase().as_str() {
            "TWELVEDATA" => "TWELVEDATA",
            _ => "BINANCE_PAXG",
        }
        .to_string();

        self.entry_mode = match self.entry_mode.trim().to_uppercase().as_str() {
            "LIMIT_RETEST" => "LIMIT_RETEST",
            "BREAKOUT" => "BREAKOUT",
            _ => "MARKET",
        }
        .to_string();
        self.broker_price_offset = finite_or(self.broker_price_offset, 0.0).clamp(-100.0, 100.0);
        // Boolean toggle: no numeric clamp needed; false is a valid explicit choice.
        self.max_entry_drift_points = finite_or(self.max_entry_drift_points, 2.5).clamp(0.5, 10.0);
        self.max_candle_age_secs = self.max_candle_age_secs.clamp(30, 900);
        self.session_alert_lead_minutes = self.session_alert_lead_minutes.clamp(15, 120);

        self.telegram_bot_token = self.telegram_bot_token.trim().to_string();
        self.telegram_chat_id = self.telegram_chat_id.trim().to_string();
        self.webhook_passphrase = self.webhook_passphrase.trim().to_string();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_dangerous_values() {
        let config = BotConfig {
            risk_percentage: 99.0,
            gold_sl_buffer_points: 1.0,
            min_risk_reward: 0.5,
            max_signals_per_day: 0,
            account_balance: 10.0,
            macro_bias: "garbage".to_string(),
            data_source: "unknown".to_string(),
            symbol: "  ".to_string(),
            ..BotConfig::default()
        }
        .sanitized();

        assert_eq!(config.risk_percentage, 5.0);
        assert_eq!(config.gold_sl_buffer_points, 5.0);
        assert_eq!(config.min_risk_reward, 1.5);
        assert_eq!(config.max_signals_per_day, 1);
        assert_eq!(config.account_balance, 100.0);
        assert_eq!(config.macro_bias, "AUTO");
        assert_eq!(config.data_source, "BINANCE_PAXG");
        assert_eq!(config.symbol, "XAU/USD");
    }

    #[test]
    fn sanitize_keeps_valid_values() {
        let config = BotConfig {
            risk_percentage: 1.5,
            gold_sl_buffer_points: 20.0,
            min_risk_reward: 2.0,
            max_signals_per_day: 1,
            macro_bias: "AUTO".to_string(),
            ..BotConfig::default()
        }
        .sanitized();

        assert_eq!(config.risk_percentage, 1.5);
        assert_eq!(config.gold_sl_buffer_points, 20.0);
        assert_eq!(config.min_risk_reward, 2.0);
        assert_eq!(config.max_signals_per_day, 1);
    }

    #[test]
    fn entry_type_serializes_uppercase() {
        assert_eq!(
            serde_json::to_string(&EntryType::Market).expect("serialize"),
            "\"MARKET\""
        );
        assert_eq!(
            serde_json::to_string(&EntryType::Limit).expect("serialize"),
            "\"LIMIT\""
        );
        assert_eq!(
            serde_json::from_str::<EntryType>("\"BREAKOUT\"").expect("deserialize"),
            EntryType::Breakout
        );
        assert_eq!(EntryType::default(), EntryType::Market);
    }

    #[test]
    fn candle_taker_buy_volume_defaults_for_legacy_json() {
        let legacy = serde_json::json!({
            "timestamp": 1_700_000_000_000_i64,
            "open": 2000.0,
            "high": 2005.0,
            "low": 1995.0,
            "close": 2001.0,
            "volume": 100.0,
            "timeframe": "M15"
        });

        let candle: Candle =
            serde_json::from_value(legacy).expect("legacy candle without taker volume");
        assert_eq!(candle.taker_buy_volume, 0.0);

        let with_taker = serde_json::json!({
            "timestamp": 1_700_000_000_000_i64,
            "open": 2000.0,
            "high": 2005.0,
            "low": 1995.0,
            "close": 2001.0,
            "volume": 100.0,
            "taker_buy_volume": 42.5,
            "timeframe": "M15"
        });
        let candle: Candle = serde_json::from_value(with_taker).expect("candle with taker volume");
        assert_eq!(candle.taker_buy_volume, 42.5);
    }

    #[test]
    fn sanitize_entry_mode_normalizes_and_rejects_invalid() {
        let limit = BotConfig {
            entry_mode: "  limit_retest ".to_string(),
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(limit.entry_mode, "LIMIT_RETEST");

        let breakout = BotConfig {
            entry_mode: "breakout".to_string(),
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(breakout.entry_mode, "BREAKOUT");

        let invalid = BotConfig {
            entry_mode: "chase".to_string(),
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(invalid.entry_mode, "MARKET");
    }

    #[test]
    fn sanitize_entry_protection_clamps_values() {
        let config = BotConfig {
            broker_price_offset: 500.0,
            max_entry_drift_points: 99.0,
            max_candle_age_secs: 5,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(config.broker_price_offset, 100.0);
        assert_eq!(config.max_entry_drift_points, 10.0);
        assert_eq!(config.max_candle_age_secs, 30);

        let negative = BotConfig {
            broker_price_offset: -500.0,
            max_entry_drift_points: 0.0,
            max_candle_age_secs: 100_000,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(negative.broker_price_offset, -100.0);
        assert_eq!(negative.max_entry_drift_points, 0.5);
        assert_eq!(negative.max_candle_age_secs, 900);

        let non_finite = BotConfig {
            broker_price_offset: f64::NAN,
            max_entry_drift_points: f64::INFINITY,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(non_finite.broker_price_offset, 0.0);
        assert_eq!(non_finite.max_entry_drift_points, 2.5);
    }

    #[test]
    fn sanitize_session_alert_settings_clamps_lead_minutes() {
        let too_low = BotConfig {
            session_alerts_enabled: false,
            session_alert_lead_minutes: 1,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(!too_low.session_alerts_enabled);
        assert_eq!(too_low.session_alert_lead_minutes, 15);

        let too_high = BotConfig {
            session_alert_lead_minutes: 999,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(too_high.session_alert_lead_minutes, 120);

        let valid = BotConfig {
            session_alert_lead_minutes: 45,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(valid.session_alert_lead_minutes, 45);
    }

    #[test]
    fn session_alert_defaults_are_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(defaults.session_alerts_enabled);
        assert_eq!(defaults.session_alert_lead_minutes, 60);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        object.remove("session_alerts_enabled");
        object.remove("session_alert_lead_minutes");

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(config.session_alerts_enabled);
        assert_eq!(config.session_alert_lead_minutes, 60);
    }

    #[test]
    fn spot_basis_defaults_true_and_is_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(defaults.use_auto_spot_basis);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        object.remove("use_auto_spot_basis");

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(config.use_auto_spot_basis);

        let disabled = BotConfig {
            use_auto_spot_basis: false,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(!disabled.use_auto_spot_basis);
    }
}

/// Dữ liệu đầu vào cho Webhook nhận nến mới từ sàn / TradingView
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCandleWebhook {
    /// Mật khẩu bảo vệ webhook (rỗng = không bắt buộc)
    #[serde(default)]
    pub passphrase: Option<String>,
    pub symbol: String,
    pub timeframe: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub timestamp: Option<i64>,
}

/// Payload webhook tiêu chuẩn tương thích TradingView Alert JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingViewWebhookPayload {
    /// Mật khẩu bảo vệ Webhook chống giả mạo
    pub passphrase: Option<String>,
    /// Mã cặp tài sản (XAUUSD, GOLD)
    pub ticker: String,
    /// Hành động lệnh (BUY, SELL, hoặc CANDLE)
    pub action: String,
    /// Giá kích hoạt
    pub price: f64,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub close: Option<f64>,
    pub volume: Option<f64>,
    pub timeframe: Option<String>,
    pub comment: Option<String>,
    pub timestamp: Option<i64>,
}

/// Báo cáo vĩ mô định kỳ 1 lần/ngày lúc 07:00 AM (AI tùy chọn: Gemini/OpenRouter,
/// luôn có fallback định lượng EMA50/200 miễn phí)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyMacroReport {
    pub date: String,
    pub macro_bias: MacroBias,
    pub confidence_score: u8,
    pub gold_range_forecast: String,
    pub daily_rationale: String,
    pub high_impact_events_today: Vec<String>,
    pub recommended_action: String,
    pub updated_at: DateTime<Utc>,
}
