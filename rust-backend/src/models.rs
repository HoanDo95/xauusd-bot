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

impl SignalType {
    /// Mã ngắn dùng cho log/filter ("BUY" | "SELL").
    pub fn as_str(&self) -> &'static str {
        match self {
            SignalType::Buy => "BUY",
            SignalType::Sell => "SELL",
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
    /// Kết luận của AI Chief Risk Officer: "BUY" | "SELL" | "STANDBY".
    /// None = AI review tắt/không chạy. AI chỉ được veto, không đặt SL/TP live.
    #[serde(default)]
    pub ai_verdict: Option<String>,
    /// Độ tin cậy AI 0.0..=1.0 (advisory).
    #[serde(default)]
    pub ai_confidence: Option<f64>,
    /// Lý do ngắn gọn từ AI Risk Officer (advisory).
    #[serde(default)]
    pub ai_reason: Option<String>,
    /// Delta ratio của cây nến tín hiệu (chỉ điền khi bật Volume Delta filter).
    #[serde(default)]
    pub delta_ratio: Option<f64>,
    /// CVD tích lũy trong cửa sổ lookback (chỉ điền khi bật Volume Delta filter).
    #[serde(default)]
    pub cvd: Option<f64>,
    /// Delta tuyệt đối của cây nến tín hiệu (chỉ điền khi bật Volume Delta filter).
    #[serde(default)]
    pub delta: Option<f64>,
    /// RSI (Wilder) của cây nến tín hiệu — advisory/context.
    #[serde(default)]
    pub rsi_value: Option<f64>,
    /// "Overbought" | "Neutral" | "Oversold".
    #[serde(default)]
    pub rsi_state: Option<String>,
    /// "None" | "Bullish" | "Bearish".
    #[serde(default)]
    pub rsi_divergence: Option<String>,
    /// Macro/flow alignment at signal time: "ALIGNED" | "COUNTER" | "MIXED" | "UNKNOWN".
    /// Advisory only — never blocks a signal.
    #[serde(default)]
    pub macro_flow_alignment: Option<String>,
    /// Short Vietnamese macro/flow note (advisory only).
    #[serde(default)]
    pub macro_flow_note: Option<String>,
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
    /// Bật bộ lọc Volume Absorption. Mặc định true; tắt khi nguồn không có volume.
    #[serde(default = "default_true")]
    pub volume_filter_enabled: bool,
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
    /// Chặn tín hiệu ngược xu hướng H4 (BUY khi H4 BEARISH / SELL khi H4 BULLISH).
    /// Mặc định TẮT — D1/H4/H1 chỉ mang tính tham khảo, không đổi hành vi mặc định.
    #[serde(default)]
    pub htf_h4_filter_enabled: bool,
    /// Chặn thêm tín hiệu ngược D1 khi bật (BUY khi D1 BEARISH / SELL khi D1 BULLISH).
    /// Mặc định TẮT.
    #[serde(default)]
    pub htf_d1_filter_enabled: bool,
    /// Bật AI Chief Risk Officer review trước khi gửi/lưu tín hiệu thật.
    /// Mặc định TẮT — không thay đổi hành vi tín hiệu hiện tại.
    #[serde(default)]
    pub ai_signal_review_enabled: bool,
    /// Ngưỡng confidence tối thiểu để cho qua (0.5..=1.0, mặc định 0.75).
    #[serde(default = "default_ai_signal_review_min_confidence")]
    pub ai_signal_review_min_confidence: f64,
    /// Khi AI lỗi/timeout: true = cho tín hiệu qua (fail-open), false = chặn.
    #[serde(default = "default_true")]
    pub ai_signal_review_fail_open: bool,
    /// Bật bộ lọc Volume Delta/CVD trước khi phát tín hiệu thật. Mặc định TẮT.
    #[serde(default)]
    pub volume_delta_filter_enabled: bool,
    /// Chế độ lọc: "ALIGN", "DIVERGENCE" hoặc "BOTH" (mặc định).
    #[serde(default = "default_volume_delta_mode")]
    pub volume_delta_mode: String,
    /// Số nến dùng cho CVD và divergence (5..=100, mặc định 20).
    #[serde(default = "default_volume_delta_lookback")]
    pub volume_delta_lookback: usize,
    /// Ngưỡng |delta_ratio| tối thiểu cho chế độ ALIGN (0..=1, mặc định 0.10).
    #[serde(default = "default_volume_delta_min_ratio")]
    pub volume_delta_min_ratio: f64,
    /// Bật bộ lọc RSI cứng trước khi phát tín hiệu thật. Mặc định TẮT.
    /// Phase R0 cho thấy RSI 70/30 tại nến trigger = 0/871 lệnh và divergence
    /// mất tiền, nên chỉ bật sau khi forward-test có bằng chứng mới.
    #[serde(default)]
    pub rsi_filter_enabled: bool,
    /// Chu kỳ Wilder RSI (2..=100, mặc định 14).
    #[serde(default = "default_rsi_period")]
    pub rsi_period: usize,
    /// Ngưỡng Overbought (50..=100, mặc định 70).
    #[serde(default = "default_rsi_overbought")]
    pub rsi_overbought: f64,
    /// Ngưỡng Oversold (0..=50, mặc định 30).
    #[serde(default = "default_rsi_oversold")]
    pub rsi_oversold: f64,
    /// Bật thành phần divergence trong bộ lọc RSI (mặc định true).
    #[serde(default = "default_rsi_divergence_enabled")]
    pub rsi_divergence_enabled: bool,
    /// Số nến nhìn lại để dò pivot divergence (10..=200, mặc định 40).
    #[serde(default = "default_rsi_divergence_lookback")]
    pub rsi_divergence_lookback: usize,
    /// Số nến xác nhận mỗi bên của pivot (1..=10, mặc định 2).
    #[serde(default = "default_rsi_pivot_k")]
    pub rsi_pivot_k: usize,
    /// Chế độ lọc: "EXTREME", "DIVERGENCE" hoặc "BOTH" (mặc định).
    #[serde(default = "default_rsi_mode")]
    pub rsi_mode: String,
    /// Bật cảnh báo phân kỳ RSI độc lập (không phải tín hiệu vào lệnh, không tiêu hao quota).
    #[serde(default = "default_true")]
    pub divergence_alert_enabled: bool,
    /// Bật cảnh báo phân kỳ RSI trên khung M15.
    #[serde(default = "default_true")]
    pub divergence_alert_m15: bool,
    /// Bật cảnh báo phân kỳ RSI trên khung H1.
    #[serde(default = "default_true")]
    pub divergence_alert_h1: bool,
    /// Bật cảnh báo phân kỳ RSI trên khung H4.
    #[serde(default = "default_true")]
    pub divergence_alert_h4: bool,
    /// Số phút tối thiểu giữa hai cảnh báo cùng khung + cùng hướng (5..=1440, mặc định 60).
    #[serde(default = "default_divergence_alert_cooldown")]
    pub divergence_alert_cooldown_minutes: i64,
    /// Bật cảnh báo Telegram khi PRIMARY scenario chạm trigger trên nến M15 đã đóng.
    /// Read-only: không phải trade signal, không tiêu hao quota. Mặc định true.
    #[serde(default = "default_true")]
    pub action_trigger_alert_enabled: bool,
    /// Số phút tối thiểu giữa hai cảnh báo cùng scenario + hướng + mức trigger
    /// (5..=1440, mặc định 30).
    #[serde(default = "default_action_trigger_cooldown")]
    pub action_trigger_cooldown_minutes: i64,
    /// Bật dashboard Macro & Flow read-only (FRED/Yahoo/CFTC). Mặc định true.
    #[serde(default = "default_true")]
    pub macro_flow_enabled: bool,
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

fn default_ai_signal_review_min_confidence() -> f64 {
    0.75
}

fn default_volume_delta_mode() -> String {
    "BOTH".to_string()
}

fn default_volume_delta_lookback() -> usize {
    20
}

fn default_volume_delta_min_ratio() -> f64 {
    0.10
}

fn default_rsi_period() -> usize {
    14
}

fn default_rsi_overbought() -> f64 {
    70.0
}

fn default_rsi_oversold() -> f64 {
    30.0
}

fn default_rsi_divergence_enabled() -> bool {
    true
}

fn default_rsi_divergence_lookback() -> usize {
    40
}

fn default_rsi_pivot_k() -> usize {
    2
}

fn default_rsi_mode() -> String {
    "BOTH".to_string()
}

fn default_divergence_alert_cooldown() -> i64 {
    60
}

fn default_action_trigger_cooldown() -> i64 {
    30
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            symbol: "XAU/USD".to_string(),
            timeframe: Timeframe::M15,
            min_risk_reward: 2.0,
            gold_sl_buffer_points: 8.5,
            volume_surge_multiplier: 1.5,
            volume_filter_enabled: true,
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
            htf_h4_filter_enabled: false,
            htf_d1_filter_enabled: false,
            ai_signal_review_enabled: false,
            ai_signal_review_min_confidence: default_ai_signal_review_min_confidence(),
            ai_signal_review_fail_open: true,
            volume_delta_filter_enabled: false,
            volume_delta_mode: default_volume_delta_mode(),
            volume_delta_lookback: default_volume_delta_lookback(),
            volume_delta_min_ratio: default_volume_delta_min_ratio(),
            rsi_filter_enabled: false,
            rsi_period: default_rsi_period(),
            rsi_overbought: default_rsi_overbought(),
            rsi_oversold: default_rsi_oversold(),
            rsi_divergence_enabled: default_rsi_divergence_enabled(),
            rsi_divergence_lookback: default_rsi_divergence_lookback(),
            rsi_pivot_k: default_rsi_pivot_k(),
            rsi_mode: default_rsi_mode(),
            divergence_alert_enabled: true,
            divergence_alert_m15: true,
            divergence_alert_h1: true,
            divergence_alert_h4: true,
            divergence_alert_cooldown_minutes: default_divergence_alert_cooldown(),
            action_trigger_alert_enabled: true,
            action_trigger_cooldown_minutes: default_action_trigger_cooldown(),
            macro_flow_enabled: true,
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
        // volume_filter_enabled is a boolean toggle; false is a valid explicit choice.
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
        self.ai_signal_review_min_confidence =
            finite_or(self.ai_signal_review_min_confidence, 0.75).clamp(0.5, 1.0);

        self.volume_delta_mode = match self.volume_delta_mode.trim().to_uppercase().as_str() {
            "ALIGN" => "ALIGN",
            "DIVERGENCE" => "DIVERGENCE",
            _ => "BOTH",
        }
        .to_string();
        self.volume_delta_lookback = self.volume_delta_lookback.clamp(5, 100);
        self.volume_delta_min_ratio = finite_or(self.volume_delta_min_ratio, 0.10).clamp(0.0, 1.0);

        self.rsi_period = self.rsi_period.clamp(2, 100);
        self.rsi_overbought = finite_or(self.rsi_overbought, 70.0).clamp(50.0, 100.0);
        self.rsi_oversold = finite_or(self.rsi_oversold, 30.0).clamp(0.0, 50.0);
        if self.rsi_oversold >= self.rsi_overbought {
            self.rsi_overbought = 70.0;
            self.rsi_oversold = 30.0;
        }
        self.rsi_divergence_lookback = self.rsi_divergence_lookback.clamp(10, 200);
        self.rsi_pivot_k = self.rsi_pivot_k.clamp(1, 10);
        self.rsi_mode = match self.rsi_mode.trim().to_uppercase().as_str() {
            "EXTREME" => "EXTREME",
            "DIVERGENCE" => "DIVERGENCE",
            _ => "BOTH",
        }
        .to_string();
        // Boolean toggles keep their explicit value; only the cooldown is clamped.
        self.divergence_alert_cooldown_minutes =
            self.divergence_alert_cooldown_minutes.clamp(5, 1440);
        self.action_trigger_cooldown_minutes = self.action_trigger_cooldown_minutes.clamp(5, 1440);

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
    fn volume_filter_defaults_true_and_is_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(defaults.volume_filter_enabled);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        object.remove("volume_filter_enabled");

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(config.volume_filter_enabled);

        let disabled = BotConfig {
            volume_filter_enabled: false,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(!disabled.volume_filter_enabled);
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
    fn htf_h4_filter_defaults_false_and_is_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(!defaults.htf_h4_filter_enabled);
        assert!(!defaults.htf_d1_filter_enabled);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        object.remove("htf_h4_filter_enabled");

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(!config.htf_h4_filter_enabled);

        let enabled = BotConfig {
            htf_h4_filter_enabled: true,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(enabled.htf_h4_filter_enabled);
    }

    #[test]
    fn ai_signal_review_defaults_off_and_is_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(!defaults.ai_signal_review_enabled);
        assert_eq!(defaults.ai_signal_review_min_confidence, 0.75);
        assert!(defaults.ai_signal_review_fail_open);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        object.remove("ai_signal_review_enabled");
        object.remove("ai_signal_review_min_confidence");
        object.remove("ai_signal_review_fail_open");

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(!config.ai_signal_review_enabled);
        assert_eq!(config.ai_signal_review_min_confidence, 0.75);
        assert!(config.ai_signal_review_fail_open);
    }

    #[test]
    fn sanitize_clamps_ai_signal_review_confidence() {
        let too_low = BotConfig {
            ai_signal_review_min_confidence: 0.1,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(too_low.ai_signal_review_min_confidence, 0.5);

        let too_high = BotConfig {
            ai_signal_review_min_confidence: 99.0,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(too_high.ai_signal_review_min_confidence, 1.0);

        let non_finite = BotConfig {
            ai_signal_review_min_confidence: f64::NAN,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(non_finite.ai_signal_review_min_confidence, 0.75);

        let valid = BotConfig {
            ai_signal_review_enabled: true,
            ai_signal_review_min_confidence: 0.82,
            ai_signal_review_fail_open: false,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(valid.ai_signal_review_enabled);
        assert_eq!(valid.ai_signal_review_min_confidence, 0.82);
        assert!(!valid.ai_signal_review_fail_open);
    }

    #[test]
    fn volume_delta_defaults_off_and_is_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(!defaults.volume_delta_filter_enabled);
        assert_eq!(defaults.volume_delta_mode, "BOTH");
        assert_eq!(defaults.volume_delta_lookback, 20);
        assert_eq!(defaults.volume_delta_min_ratio, 0.10);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        object.remove("volume_delta_filter_enabled");
        object.remove("volume_delta_mode");
        object.remove("volume_delta_lookback");
        object.remove("volume_delta_min_ratio");

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(!config.volume_delta_filter_enabled);
        assert_eq!(config.volume_delta_mode, "BOTH");
        assert_eq!(config.volume_delta_lookback, 20);
        assert_eq!(config.volume_delta_min_ratio, 0.10);
    }

    #[test]
    fn sanitize_volume_delta_settings_normalizes_and_clamps() {
        let config = BotConfig {
            volume_delta_filter_enabled: true,
            volume_delta_mode: " divergence ".to_string(),
            volume_delta_lookback: 1,
            volume_delta_min_ratio: 5.0,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(config.volume_delta_filter_enabled);
        assert_eq!(config.volume_delta_mode, "DIVERGENCE");
        assert_eq!(config.volume_delta_lookback, 5);
        assert_eq!(config.volume_delta_min_ratio, 1.0);

        let invalid = BotConfig {
            volume_delta_mode: "garbage".to_string(),
            volume_delta_lookback: 999,
            volume_delta_min_ratio: f64::NAN,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(invalid.volume_delta_mode, "BOTH");
        assert_eq!(invalid.volume_delta_lookback, 100);
        assert_eq!(invalid.volume_delta_min_ratio, 0.10);

        let zero_ratio = BotConfig {
            volume_delta_min_ratio: 0.0,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(zero_ratio.volume_delta_min_ratio, 0.0);
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

    #[test]
    fn rsi_defaults_off_and_are_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(!defaults.rsi_filter_enabled);
        assert_eq!(defaults.rsi_period, 14);
        assert_eq!(defaults.rsi_overbought, 70.0);
        assert_eq!(defaults.rsi_oversold, 30.0);
        assert!(defaults.rsi_divergence_enabled);
        assert_eq!(defaults.rsi_divergence_lookback, 40);
        assert_eq!(defaults.rsi_pivot_k, 2);
        assert_eq!(defaults.rsi_mode, "BOTH");

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        for key in [
            "rsi_filter_enabled",
            "rsi_period",
            "rsi_overbought",
            "rsi_oversold",
            "rsi_divergence_enabled",
            "rsi_divergence_lookback",
            "rsi_pivot_k",
            "rsi_mode",
        ] {
            object.remove(key);
        }

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(!config.rsi_filter_enabled, "filter must default OFF");
        assert_eq!(config.rsi_period, 14);
        assert_eq!(config.rsi_overbought, 70.0);
        assert_eq!(config.rsi_oversold, 30.0);
        assert!(config.rsi_divergence_enabled);
        assert_eq!(config.rsi_divergence_lookback, 40);
        assert_eq!(config.rsi_pivot_k, 2);
        assert_eq!(config.rsi_mode, "BOTH");
    }

    #[test]
    fn sanitize_rsi_settings_normalizes_and_clamps() {
        let config = BotConfig {
            rsi_filter_enabled: true,
            rsi_period: 1,
            rsi_overbought: 99.0,
            rsi_oversold: -5.0,
            rsi_divergence_lookback: 1,
            rsi_pivot_k: 99,
            rsi_mode: " extreme ".to_string(),
            ..BotConfig::default()
        }
        .sanitized();
        assert!(config.rsi_filter_enabled);
        assert_eq!(config.rsi_period, 2);
        assert_eq!(config.rsi_overbought, 99.0);
        assert_eq!(config.rsi_oversold, 0.0);
        assert_eq!(config.rsi_divergence_lookback, 10);
        assert_eq!(config.rsi_pivot_k, 10);
        assert_eq!(config.rsi_mode, "EXTREME");

        let invalid = BotConfig {
            rsi_period: 999,
            rsi_overbought: 150.0,
            rsi_oversold: 60.0,
            rsi_divergence_lookback: 999,
            rsi_pivot_k: 0,
            rsi_mode: "garbage".to_string(),
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(invalid.rsi_period, 100);
        // Both thresholds clamp to 100/50, which still satisfies oversold < overbought.
        assert_eq!(invalid.rsi_overbought, 100.0);
        assert_eq!(invalid.rsi_oversold, 50.0);
        assert_eq!(invalid.rsi_divergence_lookback, 200);
        assert_eq!(invalid.rsi_pivot_k, 1);
        assert_eq!(invalid.rsi_mode, "BOTH");

        let inverted = BotConfig {
            rsi_overbought: 50.0,
            rsi_oversold: 50.0,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(inverted.rsi_overbought, 70.0);
        assert_eq!(inverted.rsi_oversold, 30.0);

        let non_finite = BotConfig {
            rsi_overbought: f64::NAN,
            rsi_oversold: f64::INFINITY,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(non_finite.rsi_overbought, 70.0);
        assert_eq!(non_finite.rsi_oversold, 30.0);
    }

    #[test]
    fn divergence_alert_defaults_true_and_are_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(defaults.divergence_alert_enabled);
        assert!(defaults.divergence_alert_m15);
        assert!(defaults.divergence_alert_h1);
        assert!(defaults.divergence_alert_h4);
        assert_eq!(defaults.divergence_alert_cooldown_minutes, 60);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        for key in [
            "divergence_alert_enabled",
            "divergence_alert_m15",
            "divergence_alert_h1",
            "divergence_alert_h4",
            "divergence_alert_cooldown_minutes",
        ] {
            object.remove(key);
        }

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(config.divergence_alert_enabled, "alerts must default ON");
        assert!(config.divergence_alert_m15);
        assert!(config.divergence_alert_h1);
        assert!(config.divergence_alert_h4);
        assert_eq!(config.divergence_alert_cooldown_minutes, 60);
    }

    #[test]
    fn macro_flow_defaults_true_and_is_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(defaults.macro_flow_enabled);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        value
            .as_object_mut()
            .expect("config object")
            .remove("macro_flow_enabled");

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(config.macro_flow_enabled, "macro flow must default ON");

        let disabled = BotConfig {
            macro_flow_enabled: false,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(!disabled.macro_flow_enabled);
    }

    #[test]
    fn sanitize_divergence_alert_cooldown_clamps_but_keeps_booleans() {
        let too_low = BotConfig {
            divergence_alert_enabled: false,
            divergence_alert_m15: false,
            divergence_alert_cooldown_minutes: 0,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(!too_low.divergence_alert_enabled);
        assert!(!too_low.divergence_alert_m15);
        assert_eq!(too_low.divergence_alert_cooldown_minutes, 5);

        let too_high = BotConfig {
            divergence_alert_cooldown_minutes: 99_999,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(too_high.divergence_alert_cooldown_minutes, 1440);

        let valid = BotConfig {
            divergence_alert_cooldown_minutes: 120,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(valid.divergence_alert_cooldown_minutes, 120);
    }

    #[test]
    fn action_trigger_alert_defaults_true_and_is_backward_compatible() {
        let defaults = BotConfig::default();
        assert!(defaults.action_trigger_alert_enabled);
        assert_eq!(defaults.action_trigger_cooldown_minutes, 30);

        let mut value = serde_json::to_value(BotConfig::default()).expect("serialize config");
        let object = value.as_object_mut().expect("config object");
        object.remove("action_trigger_alert_enabled");
        object.remove("action_trigger_cooldown_minutes");

        let config: BotConfig = serde_json::from_value(value).expect("deserialize legacy config");
        assert!(
            config.action_trigger_alert_enabled,
            "alerts must default ON"
        );
        assert_eq!(config.action_trigger_cooldown_minutes, 30);

        let disabled = BotConfig {
            action_trigger_alert_enabled: false,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(!disabled.action_trigger_alert_enabled);
    }

    #[test]
    fn sanitize_action_trigger_cooldown_clamps_but_keeps_toggle() {
        let too_low = BotConfig {
            action_trigger_alert_enabled: false,
            action_trigger_cooldown_minutes: 0,
            ..BotConfig::default()
        }
        .sanitized();
        assert!(!too_low.action_trigger_alert_enabled);
        assert_eq!(too_low.action_trigger_cooldown_minutes, 5);

        let too_high = BotConfig {
            action_trigger_cooldown_minutes: 99_999,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(too_high.action_trigger_cooldown_minutes, 1440);

        let valid = BotConfig {
            action_trigger_cooldown_minutes: 45,
            ..BotConfig::default()
        }
        .sanitized();
        assert_eq!(valid.action_trigger_cooldown_minutes, 45);
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
