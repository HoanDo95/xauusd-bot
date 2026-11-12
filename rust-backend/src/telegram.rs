use crate::divergence_alerts::DivergenceAlert;
use crate::models::{EntryType, Signal, SignalStatus, SignalType, SupportResistanceZone};
use crate::session_alerts::SessionBriefing;
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
        let entry_label = signal.entry_type.as_str();
        let data_source = Self::escape_html(&signal.data_source);
        let retest_line = signal
            .retest_limit_price
            .map(|price| format!("🔁 <b>Vùng Limit Retest:</b> <code>${:.2}</code>\n", price))
            .unwrap_or_default();
        let fill_line = match signal.entry_type {
            EntryType::Limit | EntryType::Breakout => format!(
                "{} <b>Trạng thái khớp:</b> {}\n",
                if signal.is_entry_filled { "✅" } else { "⏳" },
                if signal.is_entry_filled {
                    "FILLED"
                } else {
                    "PENDING"
                }
            ),
            EntryType::Market => String::new(),
        };
        let price_action = Self::escape_html(&signal.price_action_notes);
        let volume_analysis = Self::escape_html(&signal.volume_analysis_notes);
        let rationale = Self::escape_html(&signal.full_rationale);
        let ai_review_line = signal
            .ai_verdict
            .as_deref()
            .map(|verdict| {
                let confidence = signal.ai_confidence.unwrap_or(0.0).clamp(0.0, 1.0) * 100.0;
                let reason = signal.ai_reason.as_deref().unwrap_or("");
                format!(
                    "🤖 <b>AI Risk Officer:</b> {} · confidence {:.0}%\n• {}\n",
                    Self::escape_html(verdict),
                    confidence,
                    Self::escape_html(reason)
                )
            })
            .unwrap_or_default();
        let volume_delta_line = signal
            .delta_ratio
            .map(|ratio| {
                format!(
                    "📊 <b>Volume Delta:</b> <code>{:.2}</code> | CVD: <code>{:.0}</code>\n",
                    ratio,
                    signal.cvd.unwrap_or(0.0)
                )
            })
            .unwrap_or_default();
        let rsi_line = signal
            .rsi_value
            .map(|value| {
                let state = Self::escape_html(signal.rsi_state.as_deref().unwrap_or("Neutral"));
                let divergence = signal
                    .rsi_divergence
                    .as_deref()
                    .filter(|label| !label.eq_ignore_ascii_case("none"))
                    .map(|label| format!(" · {} Divergence", Self::escape_html(label)))
                    .unwrap_or_default();
                format!(
                    "📐 <b>RSI M15:</b> <code>{:.1}</code> · {}{}\n",
                    value, state, divergence
                )
            })
            .unwrap_or_default();

        format!(
            "🚨 <b>GOLD SIGNAL ALERT | {}</b> 🚨\n\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            📊 <b>Lệnh:</b> {} <code>{}</code>\n\
            ⏱ <b>Khung:</b> <code>{}</code>\n\
            💵 <b>Entry ({}):</b> <code>${:.2}</code>\n\
            🕯 <b>Giá đóng nến Pinbar:</b> <code>${:.2}</code>\n\
            {}\
            📡 <b>Nguồn giá:</b> <code>{}</code>\n\
            {}\
            🎯 <b>Take Profit 1 (1:2):</b> <code>${:.2}</code> (+{:.1} pts | +${:.2})\n\
            🚀 <b>Take Profit 2 (1:3):</b> <code>${:.2}</code> (+{:.1} pts | +${:.2})\n\
            🛑 <b>Stop Loss:</b> <code>${:.2}</code> (-{:.1} pts | -${:.2})\n\
            ⚖️ <b>Tỷ lệ R:R:</b> <b>1:{:.2}</b>\n\
            💼 <b>Quản lý vốn đề xuất:</b> <code>{:.2} Lot</code>\n\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            🔍 <b>Phân tích Kỹ thuật & Volume:</b>\n\
            • <b>Price Action:</b> {}\n\
            • <b>Volume Absorption:</b> {}\n\
            {}\
            {}\
            • <b>Đánh giá:</b> {}\n\
            {}\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            ⚠️ <i>Quản lý rủi ro tối đa 1-2% tài khoản. Chúc bạn giao dịch kỷ luật!</i>\n\
            ⚡ <i>Engine: Rust Gold Signal Bot v1.0 (Tokio/Axum)</i>",
            symbol,
            action_icon,
            action_text,
            signal.timeframe,
            entry_label,
            signal.entry_price,
            signal.candle_close_price,
            retest_line,
            data_source,
            fill_line,
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
            volume_delta_line,
            rsi_line,
            rationale,
            ai_review_line
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
                    let body: serde_json::Value =
                        resp.json().await.map_err(|e| e.without_url().to_string())?;
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
                let err = err.without_url();
                error!("❌ Không thể kết nối tới Telegram API: {}", err);
                Err(format!("Network Error: {}", err))
            }
        }
    }

    /// Định dạng cảnh báo phân kỳ RSI — advisory, không chứa entry/SL/TP.
    pub fn format_divergence_message(alert: &DivergenceAlert) -> String {
        let direction = Self::escape_html(&alert.direction.trim().to_uppercase());
        let timeframe = Self::escape_html(&alert.timeframe);
        let symbol = Self::escape_html(&alert.symbol);
        let note = Self::escape_html(&alert.note);

        format!(
            "⚠️ <b>CẢNH BÁO PHÂN KỲ {}</b> · {}\n\
             {} · Giá {:.2}\n\
             RSI(14): {:.1}\n\
             {}\n\
             Đây là cảnh báo động lượng, KHÔNG phải điểm entry.",
            direction, timeframe, symbol, alert.price, alert.rsi, note
        )
    }

    /// Gửi cảnh báo phân kỳ RSI. Token/Chat trống = simulator thành công (Ok(1001)).
    /// Hàm này không đụng tới quota hay danh sách tín hiệu giao dịch.
    pub async fn send_divergence_alert(&self, alert: &DivergenceAlert) -> Result<i64, String> {
        if self.bot_token.is_empty() || self.chat_id.is_empty() {
            info!(
                "⚠️ [Telegram Simulator] Divergence alert {} {} (chưa cấu hình token)",
                alert.timeframe, alert.direction
            );
            return Ok(1001);
        }

        let message_text = Self::format_divergence_message(alert);
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
                    let body: serde_json::Value =
                        resp.json().await.map_err(|e| e.without_url().to_string())?;
                    let msg_id = body["result"]["message_id"].as_i64().unwrap_or(0);
                    info!(
                        "✅ Đã gửi cảnh báo phân kỳ {} {} tới Telegram! Message ID: {}",
                        alert.timeframe, alert.direction, msg_id
                    );
                    Ok(msg_id)
                } else {
                    let err_text = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Lỗi không xác định".into());
                    error!("❌ Telegram divergence API lỗi: {}", err_text);
                    Err(format!("Telegram API Error: {}", err_text))
                }
            }
            Err(err) => {
                let err = err.without_url();
                error!("❌ Không thể gửi cảnh báo phân kỳ tới Telegram: {}", err);
                Err(format!("Network Error: {}", err))
            }
        }
    }

    /// Định dạng briefing thông tin trước phiên Âu/Mỹ (không phải tín hiệu vào lệnh).
    pub fn format_session_alert(briefing: &SessionBriefing) -> String {
        let session_name = Self::escape_html(briefing.session.display_name());
        let lead_label = if briefing.lead_minutes == 60 {
            "1 GIỜ".to_string()
        } else {
            format!("{} PHÚT", briefing.lead_minutes)
        };

        let data_source = Self::escape_html(&briefing.data_source);
        let open_local = Self::escape_html(&briefing.open_local_label);
        let open_vn = Self::escape_html(&briefing.open_vn_label);
        let trend = Self::escape_html(&briefing.trend_bias.to_string());

        let support = Self::format_session_zone(briefing.nearest_support.as_ref());
        let resistance = Self::format_session_zone(briefing.nearest_resistance.as_ref());

        let events_text = if briefing.upcoming_events.is_empty() {
            "Không có tin đỏ trong 12h tới".to_string()
        } else {
            briefing
                .upcoming_events
                .iter()
                .map(|event| {
                    format!(
                        "• <b>{}</b> — {} UTC",
                        Self::escape_html(&event.title),
                        event.scheduled_time.format("%d/%m %H:%M")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        format!(
            "🌍 <b>[PRE-SESSION BRIEFING] {} TRƯỚC {}</b>\n\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            🕒 <b>Mở phiên (local):</b> <code>{}</code>\n\
            🇻🇳 <b>Giờ Việt Nam:</b> <code>{}</code>\n\
            💵 <b>Giá hiện tại:</b> <code>${:.2}</code> | 📡 <code>{}</code>\n\
            📈 <b>EMA50:</b> <code>${:.2}</code> | <b>EMA200:</b> <code>${:.2}</code>\n\
            🧭 <b>Xu hướng:</b> {}\n\
            🟢 <b>Hỗ trợ gần nhất:</b> {}\n\
            🔴 <b>Kháng cự gần nhất:</b> {}\n\
            📅 <b>Tin High-Impact USD 12h tới:</b>\n\
            {}\n\
            🎯 <b>Quota hôm nay:</b> <code>{}/{}</code> lệnh thật\n\
            ━━━━━━━━━━━━━━━━━━━━━━\n\
            ⚠️ <i>Đây chỉ là briefing tham khảo trước phiên, KHÔNG phải lệnh vào trực tiếp. Hãy chờ tín hiệu Sniper từ bot.</i>\n\
            ⚡ <i>Engine: Rust Gold Signal Bot v1.0 (Tokio/Axum)</i>",
            lead_label,
            session_name,
            open_local,
            open_vn,
            briefing.current_price,
            data_source,
            briefing.ema50,
            briefing.ema200,
            trend,
            support,
            resistance,
            events_text,
            briefing.signals_today,
            briefing.max_signals_per_day
        )
    }

    fn format_session_zone(zone: Option<&SupportResistanceZone>) -> String {
        match zone {
            Some(zone) => format!("${:.2} ({} lần chạm)", zone.price, zone.touch_count),
            None => "Chưa xác định".to_string(),
        }
    }

    /// Gửi briefing trước phiên tới Telegram. Token/Chat trống = dry-run thành công.
    pub async fn send_session_alert(&self, briefing: &SessionBriefing) -> Result<i64, String> {
        if self.bot_token.is_empty() || self.chat_id.is_empty() {
            info!(
                "⚠️ [Telegram Simulator] Session briefing {} (chưa cấu hình token)",
                briefing.dedup_key
            );
            return Ok(1001);
        }

        let message_text = Self::format_session_alert(briefing);
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
                    let body: serde_json::Value =
                        resp.json().await.map_err(|e| e.without_url().to_string())?;
                    let msg_id = body["result"]["message_id"].as_i64().unwrap_or(0);
                    info!(
                        "✅ Đã gửi session briefing {} tới Telegram! Message ID: {}",
                        briefing.dedup_key, msg_id
                    );
                    Ok(msg_id)
                } else {
                    let err_text = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Lỗi không xác định".into());
                    error!("❌ Telegram session briefing API lỗi: {}", err_text);
                    Err(format!("Telegram API Error: {}", err_text))
                }
            }
            Err(err) => {
                let err = err.without_url();
                error!("❌ Không thể gửi session briefing tới Telegram: {}", err);
                Err(format!("Network Error: {}", err))
            }
        }
    }

    /// Định dạng tin nhắn cập nhật kết quả TP1/TP2/SL.
    pub fn format_outcome_message(signal: &Signal) -> String {
        let (icon, label) = match signal.status {
            SignalStatus::Target1Hit => ("✅", "TP1 HIT"),
            SignalStatus::Target2Hit => ("🚀", "TP2 HIT"),
            SignalStatus::StopLossHit => ("🛑", "STOP LOSS HIT"),
            SignalStatus::Cancelled => ("⚪", "CANCELLED"),
            SignalStatus::Active => ("ℹ️", "UPDATE"),
        };

        let risk = (signal.entry_price - signal.stop_loss).abs();
        let r = if risk > f64::EPSILON {
            match signal.status {
                SignalStatus::Target1Hit => {
                    (signal.take_profit_1 - signal.entry_price).abs() / risk
                }
                SignalStatus::Target2Hit => {
                    (signal.take_profit_2 - signal.entry_price).abs() / risk
                }
                SignalStatus::StopLossHit => -1.0,
                _ => 0.0,
            }
        } else {
            0.0
        };

        let outcome_time = signal
            .outcome_at
            .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "vừa xong".to_string());

        format!(
            "{} <b>{} | {} {}</b>\n\
             ━━━━━━━━━━━━━━━━━━━━━━\n\
             🆔 <code>{}</code>\n\
             💵 Entry: <code>${:.2}</code>\n\
             🎯 TP1: <code>${:.2}</code> | 🚀 TP2: <code>${:.2}</code> | 🛑 SL: <code>${:.2}</code>\n\
             📊 Kết quả: <b>{:+.2}R</b>\n\
             ⏱ Thời điểm: {}\n\
             ━━━━━━━━━━━━━━━━━━━━━━\n\
             ⚠️ <i>Quản lý rủi ro kỷ luật. Đây không phải lời khuyên đầu tư.</i>",
            icon,
            label,
            Self::escape_html(&signal.symbol),
            signal.signal_type,
            signal.id,
            signal.entry_price,
            signal.take_profit_1,
            signal.take_profit_2,
            signal.stop_loss,
            r,
            outcome_time
        )
    }

    /// Gửi thông báo TP1/TP2/SL khi trạng thái tín hiệu thay đổi.
    pub async fn send_outcome_alert(&self, signal: &Signal) -> Result<i64, String> {
        if signal.status == SignalStatus::Active {
            // Chỉ cập nhật trạng thái khớp lệnh Limit/Breakout, không spam Telegram.
            return Ok(1001);
        }
        if self.bot_token.is_empty() || self.chat_id.is_empty() {
            info!(
                "⚠️ [Telegram Simulator] Outcome {} (chưa cấu hình token)",
                signal.id
            );
            return Ok(1001);
        }

        let text = Self::format_outcome_message(signal);
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        let payload = json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true
        });

        match self.client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    let body: serde_json::Value =
                        resp.json().await.map_err(|e| e.without_url().to_string())?;
                    let msg_id = body["result"]["message_id"].as_i64().unwrap_or(0);
                    info!("✅ Đã gửi outcome Telegram! Message ID: {}", msg_id);
                    Ok(msg_id)
                } else {
                    let err_text = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Lỗi không xác định".into());
                    error!("❌ Telegram outcome API lỗi: {}", err_text);
                    Err(format!("Telegram API Error: {}", err_text))
                }
            }
            Err(err) => {
                let err = err.without_url();
                error!("❌ Không thể gửi outcome tới Telegram: {}", err);
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
            Err(e) => Err(format!("Lỗi kết nối mạng: {}", e.without_url())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EconomicEvent, EntryType, MacroBias, SignalStatus, Timeframe, ZoneType};
    use crate::news_filter::ImpactLevel;
    use crate::session_alerts::TradingSession;
    use chrono::{Duration, Utc};

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
            outcome_at: None,
            entry_type: EntryType::Market,
            candle_close_price: 2684.50,
            live_market_price: Some(2684.50),
            retest_limit_price: Some(2680.00),
            data_source: "TWELVEDATA".to_string(),
            is_entry_filled: true,
            ai_verdict: None,
            ai_confidence: None,
            ai_reason: None,
            delta_ratio: None,
            cvd: None,
            delta: None,
            rsi_value: None,
            rsi_state: None,
            rsi_divergence: None,
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
    fn format_outcome_message_labels_tp1() {
        let mut signal = sample_signal();
        signal.status = SignalStatus::Target1Hit;
        signal.outcome_at = Some(Utc::now());

        let message = TelegramNotifier::format_outcome_message(&signal);

        assert!(message.contains("TP1 HIT"));
        assert!(message.contains("+2.00R"));
        assert!(!message.contains("<script>"));
    }

    #[test]
    fn format_signal_message_escapes_dynamic_fields() {
        let mut signal = sample_signal();
        signal.symbol = "XAU<USD>&".to_string();
        signal.price_action_notes = "break <script> & test".to_string();
        signal.data_source = "SRC<&>".to_string();

        let message = TelegramNotifier::format_signal_message(&signal);

        assert!(message.contains("XAU&lt;USD&gt;&amp;"));
        assert!(!message.contains("<script>"));
        assert!(message.contains("&lt;script&gt;"));
        assert!(message.contains("&amp; test"));
        assert!(message.contains("SRC&lt;&amp;&gt;"));
        assert!(!message.contains("SRC<&>"));
    }

    #[test]
    fn format_signal_message_appends_escaped_ai_review() {
        let mut signal = sample_signal();
        signal.ai_verdict = Some("STANDBY<script>".to_string());
        signal.ai_confidence = Some(0.87);
        signal.ai_reason = Some("risk <high> & red news".to_string());

        let message = TelegramNotifier::format_signal_message(&signal);

        assert!(message.contains("AI Risk Officer:"));
        assert!(message.contains("STANDBY&lt;script&gt;"));
        assert!(message.contains("confidence 87%"));
        assert!(message.contains("risk &lt;high&gt; &amp; red news"));
        assert!(!message.contains("STANDBY<script>"));
    }

    #[test]
    fn format_signal_message_omits_ai_review_when_absent() {
        let signal = sample_signal();
        let message = TelegramNotifier::format_signal_message(&signal);
        assert!(!message.contains("AI Risk Officer"));
    }

    #[test]
    fn format_signal_message_includes_volume_delta_when_present() {
        let mut signal = sample_signal();
        signal.delta_ratio = Some(0.42);
        signal.cvd = Some(1250.0);
        signal.delta = Some(420.0);

        let message = TelegramNotifier::format_signal_message(&signal);
        assert!(message.contains("Volume Delta:"));
        assert!(message.contains("0.42"));
        assert!(message.contains("CVD: <code>1250</code>"));

        let without = TelegramNotifier::format_signal_message(&sample_signal());
        assert!(!without.contains("Volume Delta:"));
    }

    #[test]
    fn format_signal_message_appends_rsi_line_and_divergence() {
        let mut signal = sample_signal();
        signal.rsi_value = Some(76.4);
        signal.rsi_state = Some("Overbought".to_string());
        signal.rsi_divergence = Some("Bearish".to_string());

        let message = TelegramNotifier::format_signal_message(&signal);
        assert!(message.contains("RSI M15:"));
        assert!(message.contains("76.4"));
        assert!(message.contains("Overbought"));
        assert!(message.contains("Bearish Divergence"));

        signal.rsi_divergence = Some("None".to_string());
        let without_divergence = TelegramNotifier::format_signal_message(&signal);
        assert!(without_divergence.contains("RSI M15:"));
        assert!(!without_divergence.contains("None Divergence"));

        let no_rsi = TelegramNotifier::format_signal_message(&sample_signal());
        assert!(!no_rsi.contains("RSI M15:"));
    }

    #[test]
    fn format_signal_message_escapes_rsi_state() {
        let mut signal = sample_signal();
        signal.rsi_value = Some(50.0);
        signal.rsi_state = Some("Neutral<script>".to_string());
        signal.rsi_divergence = Some("Bullish<&>".to_string());

        let message = TelegramNotifier::format_signal_message(&signal);
        assert!(message.contains("Neutral&lt;script&gt;"));
        assert!(message.contains("Bullish&lt;&amp;&gt; Divergence"));
        assert!(!message.contains("Neutral<script>"));
    }

    #[test]
    fn format_signal_message_includes_entry_type_and_source() {
        let mut signal = sample_signal();
        signal.entry_type = EntryType::Limit;
        signal.candle_close_price = 2684.50;
        signal.live_market_price = Some(2686.00);
        signal.retest_limit_price = Some(2680.00);
        signal.is_entry_filled = false;
        signal.data_source = "BINANCE_PAXG".to_string();

        let message = TelegramNotifier::format_signal_message(&signal);

        assert!(message.contains("Entry (LIMIT):"));
        assert!(message.contains("$2684.50"));
        assert!(message.contains("Giá đóng nến Pinbar:"));
        assert!(message.contains("Vùng Limit Retest:"));
        assert!(message.contains("$2680.00"));
        assert!(message.contains("Nguồn giá:"));
        assert!(message.contains("BINANCE_PAXG"));
        assert!(message.contains("Trạng thái khớp:"));
        assert!(message.contains("PENDING"));
        assert!(!message.contains("<script>"));
    }

    #[test]
    fn format_signal_message_market_entry_hides_pending_status() {
        let signal = sample_signal();
        let message = TelegramNotifier::format_signal_message(&signal);

        assert!(message.contains("Entry (MARKET):"));
        assert!(!message.contains("Trạng thái khớp:"));
    }

    fn sample_session_briefing() -> SessionBriefing {
        SessionBriefing {
            session: TradingSession::London,
            dedup_key: "SESSION_LONDON_2026-11-09".to_string(),
            open_local_label: "08:00 Europe/London (GMT)".to_string(),
            open_vn_label: "15:00 VN (UTC+7)".to_string(),
            lead_minutes: 60,
            current_price: 2684.50,
            data_source: "SRC<&>".to_string(),
            ema50: 2678.20,
            ema200: 2650.90,
            trend_bias: MacroBias::Bullish,
            nearest_support: Some(SupportResistanceZone {
                level_type: ZoneType::Support,
                price: 2670.00,
                min_price: 2668.00,
                max_price: 2672.00,
                touch_count: 3,
                timeframe: Timeframe::M15,
                strength_score: 0.75,
            }),
            nearest_resistance: Some(SupportResistanceZone {
                level_type: ZoneType::Resistance,
                price: 2695.00,
                min_price: 2693.00,
                max_price: 2697.00,
                touch_count: 2,
                timeframe: Timeframe::M15,
                strength_score: 0.5,
            }),
            upcoming_events: vec![EconomicEvent {
                id: "EVT-CPI".to_string(),
                title: "US <CPI> & <script>".to_string(),
                currency: "USD".to_string(),
                impact: ImpactLevel::High,
                scheduled_time: Utc::now() + Duration::hours(2),
                forecast: Some("0.3%".to_string()),
                previous: Some("0.2%".to_string()),
                actual: None,
            }],
            signals_today: 1,
            max_signals_per_day: 1,
        }
    }

    #[test]
    fn format_session_alert_escapes_dynamic_fields() {
        let briefing = sample_session_briefing();
        let message = TelegramNotifier::format_session_alert(&briefing);

        assert!(message.contains("PRE-SESSION BRIEFING] 1 GIỜ TRƯỚC PHIÊN ÂU (LONDON)"));
        assert!(message.contains("08:00 Europe/London (GMT)"));
        assert!(message.contains("15:00 VN (UTC+7)"));
        assert!(message.contains("$2684.50"));
        assert!(message.contains("SRC&lt;&amp;&gt;"));
        assert!(message.contains("US &lt;CPI&gt; &amp; &lt;script&gt;"));
        assert!(message.contains("KHÔNG phải lệnh vào trực tiếp"));
        assert!(!message.contains("<script>"));

        // Sau khi bỏ các thẻ HTML định dạng hợp lệ, không còn '<' thô nào.
        let stripped = message
            .replace("<b>", "")
            .replace("</b>", "")
            .replace("<i>", "")
            .replace("</i>", "")
            .replace("<code>", "")
            .replace("</code>", "");
        assert!(!stripped.contains('<'));
    }

    #[test]
    fn format_session_alert_shows_empty_news_message() {
        let mut briefing = sample_session_briefing();
        briefing.upcoming_events.clear();

        let message = TelegramNotifier::format_session_alert(&briefing);
        assert!(message.contains("Không có tin đỏ trong 12h tới"));
    }

    fn sample_divergence_alert() -> DivergenceAlert {
        DivergenceAlert {
            id: "DIV-H4-Bearish-1".to_string(),
            symbol: "XAU/USD".to_string(),
            timeframe: "H4".to_string(),
            direction: "Bearish".to_string(),
            price: 2412.34,
            rsi: 68.2,
            candle_timestamp: 1_700_000_000_000,
            detected_at: Utc::now(),
            note: "Giá tạo đỉnh cao hơn nhưng RSI tạo đỉnh thấp hơn.".to_string(),
        }
    }

    #[test]
    fn format_divergence_message_is_advisory_and_has_no_entry_levels() {
        let message = TelegramNotifier::format_divergence_message(&sample_divergence_alert());

        assert!(message.contains("CẢNH BÁO PHÂN KỲ BEARISH"));
        assert!(message.contains("· H4"));
        assert!(message.contains("XAU/USD · Giá 2412.34"));
        assert!(message.contains("RSI(14): 68.2"));
        assert!(message.contains("Giá tạo đỉnh cao hơn nhưng RSI tạo đỉnh thấp hơn."));
        assert!(message.contains("KHÔNG phải điểm entry"));
        assert!(!message.contains("Entry ("));
        assert!(!message.contains("Stop Loss"));
        assert!(!message.contains("Take Profit"));
    }

    #[test]
    fn format_divergence_message_escapes_dynamic_fields() {
        let mut alert = sample_divergence_alert();
        alert.symbol = "XAU<USD>&".to_string();
        alert.timeframe = "H4<script>".to_string();
        alert.direction = "Bear<ish>".to_string();
        alert.note = "a < b & c > d".to_string();

        let message = TelegramNotifier::format_divergence_message(&alert);

        assert!(message.contains("BEAR&lt;ISH&gt;"));
        assert!(message.contains("H4&lt;script&gt;"));
        assert!(message.contains("XAU&lt;USD&gt;&amp;"));
        assert!(message.contains("a &lt; b &amp; c &gt; d"));
        assert!(!message.contains("<script>"));
        assert!(!message.contains("Bear<ish>"));
    }
}
