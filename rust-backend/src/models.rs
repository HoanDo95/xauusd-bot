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
}

fn default_max_signals_per_day() -> usize {
    1
}

fn default_session_filter_enabled() -> bool {
    true
}

fn default_data_source() -> String {
    "BINANCE_PAXG".to_string()
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
            macro_bias: "BULLISH".to_string(),
            order_block_filter: true,
            max_signals_per_day: default_max_signals_per_day(),
            session_filter_enabled: default_session_filter_enabled(),
            data_source: default_data_source(),
        }
    }
}

/// Dữ liệu đầu vào cho Webhook nhận nến mới từ sàn / TradingView
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCandleWebhook {
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

/// Báo cáo vĩ mô định kỳ 1 lần/ngày lúc 07:00 AM từ AI (Claude/GPT via OpenRouter)
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
