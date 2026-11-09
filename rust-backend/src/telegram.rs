use crate::models::{Signal, SignalType};
use reqwest::Client;
use serde_json::json;
use tracing::{error, info};

#[derive(Debug, Clone)]
pub struct TelegramNotifier {
    client: Client,
    bot_token: String,
    chat_id: String,
}

impl TelegramNotifier {
    pub fn new(bot_token: String, chat_id: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            bot_token,
            chat_id,
        }
    }

    /// Escape các ký tự đặc biệt của Telegram HTML parse mode.
    /// Bắt buộc với mọi dữ liệu động (ticker, comment webhook, ghi chú phân tích),
    /// nếu không Telegram sẽ trả `400 Bad Request: can't parse entities`.
    pub(crate) fn escape_html(input: &str) -> String {
        input
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// Định dạng tin nhắn chuẩn và chuyên nghiệp cho Trader Vàng
    pub fn format_signal_message(signal: &Signal) -> String {
        let (action_icon, action_text) = match signal.signal_type {
            SignalType::Buy => ("🟢", "BUY / LONG"),
            SignalType::Sell => ("🔴", "SELL / SHORT"),
        };

        let symbol = Self::escape_html(&signal.symbol);
        let price_action = Self::escape_html(&signal.price_action_notes);
        let volume_analysis = Self::escape_html(&signal.volume_analysis_notes);
        let rationale = Self::escape_html(&signal.full_rationale);

        format!(
            "🚨 <b>GOLD SIGNAL ALERT | {}</b> 🚨\n\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            📊 <b>Lệnh:</b> {} <code>{}</code>\n\
            ⏱ <b>Khung:</b> <code>{}</code>\n\
            💵 <b>Entry:</b> <code>${:.2}</code>\n\
            🎯 <b>Take Profit 1 (1:2):</b> <code>${:.2}</code> (+{:.1} pts | +${:.2})\n\
            🚀 <b>Take Profit 2 (1:3):</b> <code>${:.2}</code> (+{:.1} pts | +${:.2})\n\
            🛑 <b>Stop Loss:</b> <code>${:.2}</code> (-{:.1} pts | -${:.2})\n\
            ⚖️ <b>Tỷ lệ R:R:</b> <b>1:{:.2}</b>\n\
            💼 <b>Quản lý vốn đề xuất:</b> <code>{:.2} Lot</code>\n\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            🔍 <b>Phân tích Kỹ thuật & Volume:</b>\n\
            • <b>Price Action:</b> {}\n\
            • <b>Volume Absorption:</b> {}\n\
            • <b>Đánh giá:</b> {}\n\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            ⚠️ <i>Quản lý rủi ro tối đa 1-2% tài khoản. Chúc bạn giao dịch kỷ luật!</i>\n\
            ⚡ <i>Engine: Rust Gold Signal Bot v1.0 (Tokio/Axum)</i>",
            symbol,
            action_icon,
            action_text,
            signal.timeframe,
            signal.entry_price,
            signal.take_profit_1,
            signal.reward_points_tp1,
            signal.potential_profit_tp1_usd,
            signal.take_profit_2,
            signal.reward_points_tp2,
            signal.potential_profit_tp2_usd,
            signal.stop_loss,
            signal.risk_points,
            signal.risk_amount_usd,
            signal.risk_reward_ratio,
            signal.recommended_lot_size,
            price_action,
            volume_analysis,
            rationale
        )
    }

    /// Gửi thông báo tín hiệu tới Telegram Channel / Group / DM
    pub async fn send_signal_alert(&self, signal: &Signal) -> Result<i64, String> {
        if self.bot_token.is_empty() || self.chat_id.is_empty() {
            let msg = "Telegram Bot Token hoặc Chat ID chưa được cấu hình!".to_string();
            info!("⚠️ [Telegram Simulator] {}", msg);
            return Ok(1001); // Giả lập thành công khi test local
        }

        let message_text = Self::format_signal_message(signal);
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);

        let payload = json!({
            "chat_id": self.chat_id,
            "text": message_text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true
        });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
                    let msg_id = body["result"]["message_id"].as_i64().unwrap_or(0);
                    info!(
                        "✅ Đã gửi tín hiệu thành công tới Telegram! Message ID: {}",
                        msg_id
                    );
                    Ok(msg_id)
                } else {
                    let err_text = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Lỗi không xác định".into());
                    error!("❌ Telegram API phản hồi lỗi: {}", err_text);
                    Err(format!("Telegram API Error: {}", err_text))
                }
            }
            Err(err) => {
                error!("❌ Không thể kết nối tới Telegram API: {}", err);
                Err(format!("Network Error: {}", err))
            }
        }
    }

    /// Gửi tin nhắn kiểm tra kết nối (Test Connection)
    pub async fn send_test_ping(&self) -> Result<String, String> {
        if self.bot_token.is_empty() || self.chat_id.is_empty() {
            return Err("Vui lòng điền Bot Token và Chat ID trước khi test!".to_string());
        }

        let text = "🔔 <b>[Gold Signal Bot - Rust Engine]</b>\n\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            ✅ Kết nối Telegram Bot API thành công!\n\
            Hệ thống phân tích XAU/USD M15/H1 đã sẵn sàng quét tín hiệu và tự động gửi cảnh báo.";

        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        let payload = json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": "HTML"
        });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    Ok("Gửi tin nhắn test thành công tới Telegram!".to_string())
                } else {
                    let err = resp.text().await.unwrap_or_default();
                    Err(format!("Lỗi từ Telegram API: {}", err))
                }
            }
            Err(e) => Err(format!("Lỗi kết nối mạng: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SignalStatus, Timeframe};
    use chrono::Utc;

    fn sample_signal() -> Signal {
        Signal {
            id: "SIG-TEST-TELEGRAM".to_string(),
            symbol: "XAU/USD".to_string(),
            signal_type: SignalType::Buy,
            timeframe: Timeframe::M15,
            entry_price: 2684.50,
            stop_loss: 2674.00,
            take_profit_1: 2705.50,
            take_profit_2: 2716.00,
            risk_points: 10.50,
            reward_points_tp1: 21.00,
            reward_points_tp2: 31.50,
            risk_reward_ratio: 2.00,
            volume_surge_ratio: 1.85,
            recommended_lot_size: 0.09,
            risk_amount_usd: 94.50,
            potential_profit_tp1_usd: 189.00,
            potential_profit_tp2_usd: 283.50,
            price_action_notes: "Bullish Pinbar".to_string(),
            volume_analysis_notes: "Volume 1.85x MA20".to_string(),
            full_rationale: "R:R 1:2.0".to_string(),
            status: SignalStatus::Active,
            created_at: Utc::now(),
            telegram_message_id: None,
            dedup_key: "XAU/USD_M15_1".to_string(),
            candle_timestamp: 1,
            is_simulated: false,
            confluence_score: 90,
        }
    }

    #[test]
    fn escape_html_replaces_special_chars() {
        assert_eq!(
            TelegramNotifier::escape_html("a < b & c > d"),
            "a &lt; b &amp; c &gt; d"
        );
    }

    #[test]
    fn format_signal_message_escapes_dynamic_fields() {
        let mut signal = sample_signal();
        signal.symbol = "XAU<USD>&".to_string();
        signal.price_action_notes = "break <script> & test".to_string();

        let message = TelegramNotifier::format_signal_message(&signal);

        assert!(message.contains("XAU&lt;USD&gt;&amp;"));
        assert!(!message.contains("<script>"));
        assert!(message.contains("&lt;script&gt;"));
        assert!(message.contains("&amp; test"));
    }
}
