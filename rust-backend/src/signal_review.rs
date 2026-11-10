use crate::market_context::{MarketContext, TimeframeContext};
use crate::models::{BotConfig, EconomicEvent, ImpactLevel, Signal};
use chrono::{DateTime, Duration, Timelike, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
use tracing::{info, warn};

/// OpenCode Go là provider duy nhất cho AI Risk Officer (opt-in).
const OPENCODE_GO_API_BASE: &str = "https://opencode.ai/inference/go/openai/v1";
const DEFAULT_REVIEW_MODEL: &str = "gemini-4-argon";
const OPENCODE_GO_USER_AGENT: &str = "xauusd-bot/1.0";
const REVIEW_TIMEOUT_SECS: u64 = 15;

/// Kết quả review của AI Chief Risk Officer.
///
/// Đây là dữ liệu **advisory**: `dynamic_sl` / `tp` chỉ để tham khảo, KHÔNG bao giờ
/// được ghi đè Stop Loss / Take Profit của tín hiệu thật.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSignalReview {
    /// "BUY" | "SELL" | "STANDBY"
    pub action: String,
    /// 0.0..=1.0
    pub confidence_score: f64,
    pub entry_zone: Option<String>,
    pub dynamic_sl: Option<f64>,
    pub tp: Option<f64>,
    pub macro_trend_h1_h4: Option<String>,
    pub reason: String,
    pub provider: String,
    pub model: String,
}

struct ReviewSettings {
    api_key: String,
    api_base: String,
    model: String,
}

/// Resolve cấu hình review từ env. Trả None khi thiếu API key.
/// Tuyệt đối không log key.
fn resolve_review_settings(key: &str, base: &str, model: &str) -> Option<ReviewSettings> {
    let api_key = key.trim();
    if api_key.is_empty() {
        return None;
    }

    let api_base = {
        let trimmed = base.trim();
        if trimmed.is_empty() {
            OPENCODE_GO_API_BASE.to_string()
        } else {
            trimmed.to_string()
        }
    };
    let model = {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            DEFAULT_REVIEW_MODEL.to_string()
        } else {
            trimmed.to_string()
        }
    };

    Some(ReviewSettings {
        api_key: api_key.to_string(),
        api_base,
        model,
    })
}

/// Session id gửi kèm OpenCode Go request (một phiên review/ngày UTC).
fn review_session_id(now: DateTime<Utc>) -> String {
    now.format("xauusd-review-%Y-%m-%d").to_string()
}

fn review_min_confidence(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.5, 1.0)
    } else {
        0.75
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}…", truncated)
    } else {
        truncated
    }
}

fn format_timeframe_context(label: &str, context: &TimeframeContext) -> String {
    let support = context
        .nearest_support
        .map(|price| format!("{:.2}", price))
        .unwrap_or_else(|| "n/a".to_string());
    let resistance = context
        .nearest_resistance
        .map(|price| format!("{:.2}", price))
        .unwrap_or_else(|| "n/a".to_string());

    format!(
        "{}: bias={} close={:.2} EMA50={:.2} EMA200={:.2} channel_pos={:.2} ATR14={:.2} support={} resistance={} candles={}",
        label,
        context.bias,
        context.close,
        context.ema50,
        context.ema200,
        context.channel_position,
        context.atr14,
        support,
        resistance,
        context.candles
    )
}

/// Prompt gọn: chỉ dữ liệu cần cho quyết định veto/standby.
fn build_review_prompt(
    candidate: &Signal,
    context: Option<&MarketContext>,
    events: &[EconomicEvent],
    now: DateTime<Utc>,
) -> String {
    let vn_now = now + Duration::hours(7);
    let session_label = if (13..=23).contains(&vn_now.hour()) {
        "IN golden session 13:00-23:00 VN"
    } else {
        "OUTSIDE golden session 13:00-23:00 VN"
    };

    let context_lines = match context {
        Some(context) => format!(
            "{}\n{}\n{}\nprice_source={} spot_basis={}",
            format_timeframe_context("D1", &context.d1),
            format_timeframe_context("H4", &context.h4),
            format_timeframe_context("H1", &context.h1),
            context.price_source,
            context
                .spot_basis
                .map(|basis| format!("{:+.2}", basis))
                .unwrap_or_else(|| "n/a".to_string())
        ),
        None => "Market context unavailable (HTF feed not ready).".to_string(),
    };

    let upcoming_events: Vec<String> = events
        .iter()
        .filter(|event| {
            event.impact == ImpactLevel::High
                && event.currency.eq_ignore_ascii_case("USD")
                && event.scheduled_time >= now
        })
        .take(5)
        .map(|event| {
            format!(
                "{} @ {} UTC (forecast={})",
                event.title,
                event.scheduled_time.format("%d/%m %H:%M"),
                event.forecast.as_deref().unwrap_or("n/a")
            )
        })
        .collect();
    let events_text = if upcoming_events.is_empty() {
        "none".to_string()
    } else {
        upcoming_events.join("; ")
    };

    let current_price = candidate.live_market_price.unwrap_or(candidate.entry_price);

    format!(
        "You are the independent Chief Risk Officer (AI) for a XAU/USD signal bot.\n\
        Your role is VETO-ONLY. You may approve the candidate direction, recommend STANDBY, \
        or contradict it. You must NEVER invent or change live SL/TP; dynamic_sl and tp are \
        advisory observations only.\n\n\
        Market context (read-only):\n{}\n\n\
        Candidate signal:\n\
        side={} entry={:.2} SL={:.2} TP1={:.2} TP2={:.2} RR=1:{:.2} lot={:.2}\n\
        volume_surge={:.2}x entry_type={} candle_close={:.2} current_price={:.2}\n\
        price_action={}\n\n\
        Timing: {} UTC | {} (VN) | {}\n\
        Upcoming High-Impact USD events: {}\n\n\
        Risk rules of thumb: reject counter-D1/H4 trades, stretched channel entries, \
        thin R:R, and signals immediately before red news. Be conservative but concise.\n\n\
        Return ONLY one valid JSON object with exactly these keys:\n\
        {{\"action\":\"BUY\"|\"SELL\"|\"STANDBY\",\"confidence_score\":0.0,\
        \"entry_zone\":\"short text or null\",\"dynamic_sl\":null,\"tp\":null,\
        \"macro_trend_h1_h4\":\"short text or null\",\"reason\":\"non-empty, <=240 chars\"}}\n\
        action must be uppercase BUY, SELL or STANDBY. confidence_score must be 0.0-1.0.",
        context_lines,
        candidate.signal_type.as_str(),
        candidate.entry_price,
        candidate.stop_loss,
        candidate.take_profit_1,
        candidate.take_profit_2,
        candidate.risk_reward_ratio,
        candidate.recommended_lot_size,
        candidate.volume_surge_ratio,
        candidate.entry_type.as_str(),
        candidate.candle_close_price,
        current_price,
        truncate_chars(&candidate.price_action_notes, 200),
        now.format("%Y-%m-%d %H:%M"),
        vn_now.format("%Y-%m-%d %H:%M"),
        session_label,
        events_text
    )
}

fn normalize_json_content(content: &str) -> &str {
    let trimmed = content.trim();
    let without_open = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("```JSON") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest
    } else {
        trimmed
    };
    let without_close = without_open
        .trim()
        .strip_suffix("```")
        .unwrap_or(without_open.trim());
    without_close.trim()
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn optional_f64(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
}

/// Parse + validate JSON do AI trả về. Malformed => None.
fn parse_review_content(content: &str, provider: &str, model: &str) -> Option<AiSignalReview> {
    let value: Value = serde_json::from_str(normalize_json_content(content)).ok()?;

    let action = value.get("action")?.as_str()?.trim().to_uppercase();
    if !matches!(action.as_str(), "BUY" | "SELL" | "STANDBY") {
        return None;
    }

    let confidence = value.get("confidence_score")?.as_f64()?;
    if !confidence.is_finite() {
        return None;
    }

    let reason = value.get("reason")?.as_str()?.trim();
    if reason.is_empty() {
        return None;
    }

    Some(AiSignalReview {
        action,
        confidence_score: confidence.clamp(0.0, 1.0),
        entry_zone: optional_string(&value, "entry_zone"),
        dynamic_sl: optional_f64(&value, "dynamic_sl"),
        tp: optional_f64(&value, "tp"),
        macro_trend_h1_h4: optional_string(&value, "macro_trend_h1_h4"),
        reason: reason.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
    })
}

/// Quyết định AI review có chặn tín hiệu hay không (pure, không network).
pub fn review_blocks_signal(
    candidate: &Signal,
    review: &AiSignalReview,
    min_confidence: f64,
) -> (bool, String) {
    let action = review.action.trim().to_uppercase();
    let threshold = review_min_confidence(min_confidence);

    if action == "STANDBY" {
        return (
            true,
            format!(
                "AI Risk Officer chọn STANDBY (confidence {:.0}%): {}",
                review.confidence_score * 100.0,
                review.reason
            ),
        );
    }

    if action != candidate.signal_type.as_str() {
        return (
            true,
            format!(
                "AI Risk Officer phản đối {} (verdict {}, confidence {:.0}%): {}",
                candidate.signal_type.as_str(),
                action,
                review.confidence_score * 100.0,
                review.reason
            ),
        );
    }

    if !review.confidence_score.is_finite() {
        return (
            true,
            "AI confidence không hợp lệ (non-finite) — chặn để an toàn.".to_string(),
        );
    }

    if review.confidence_score < threshold {
        return (
            true,
            format!(
                "AI confidence {:.0}% thấp hơn ngưỡng {:.0}%: {}",
                review.confidence_score * 100.0,
                threshold * 100.0,
                review.reason
            ),
        );
    }

    (
        false,
        format!(
            "AI Risk Officer đồng ý {} (confidence {:.0}%): {}",
            action,
            review.confidence_score * 100.0,
            review.reason
        ),
    )
}

/// Gọi OpenCode Go để review một tín hiệu thật.
///
/// Trả None khi AI review tắt, thiếu `OPENCODE_GO_API_KEY`, lỗi mạng/timeout,
/// HTTP lỗi hoặc JSON không hợp lệ. Caller quyết định fail-open/fail-closed.
pub async fn review_signal(
    candidate: &Signal,
    context: Option<&MarketContext>,
    config: &BotConfig,
    events: &[EconomicEvent],
) -> Option<AiSignalReview> {
    if !config.ai_signal_review_enabled {
        return None;
    }

    let settings = resolve_review_settings(
        &std::env::var("OPENCODE_GO_API_KEY").unwrap_or_default(),
        &std::env::var("OPENCODE_GO_BASE_URL").unwrap_or_default(),
        &std::env::var("AI_SIGNAL_REVIEW_MODEL").unwrap_or_default(),
    )?;

    let now = Utc::now();
    let prompt = build_review_prompt(candidate, context, events, now);
    let url = format!(
        "{}/chat/completions",
        settings.api_base.trim_end_matches('/')
    );
    let payload = json!({
        "model": settings.model,
        "messages": [
            {
                "role": "system",
                "content": "You are the independent Chief Risk Officer for an XAU/USD signal bot. You can only approve, veto (STANDBY) or contradict a candidate. Return pure valid JSON only, with no markdown wrapping."
            },
            { "role": "user", "content": prompt }
        ],
        "response_format": { "type": "json_object" },
        "temperature": 0.2
    });

    let client = Client::builder()
        .timeout(StdDuration::from_secs(REVIEW_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|_| Client::new());

    let request = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", settings.api_key))
        .header("Content-Type", "application/json")
        .header("User-Agent", OPENCODE_GO_USER_AGENT)
        .header("x-opencode-session", review_session_id(now));

    let response = match request.json(&payload).send().await {
        Ok(response) => response,
        Err(err) => {
            warn!(
                "⚠️ [AI Risk Officer] Không kết nối được OpenCode Go ({}).",
                err
            );
            return None;
        }
    };

    if !response.status().is_success() {
        warn!(
            "⚠️ [AI Risk Officer] OpenCode Go trả HTTP {}.",
            response.status()
        );
        return None;
    }

    let body = match response.json::<Value>().await {
        Ok(body) => body,
        Err(err) => {
            warn!("⚠️ [AI Risk Officer] Response JSON lỗi: {}", err);
            return None;
        }
    };

    let Some(content) = body["choices"][0]["message"]["content"].as_str() else {
        warn!("⚠️ [AI Risk Officer] Response thiếu choices[0].message.content.");
        return None;
    };

    let review = parse_review_content(content, "opencode_go", &settings.model);
    match review.as_ref() {
        Some(review) => info!(
            "🤖 [AI Risk Officer] verdict={} confidence={:.2} model={}",
            review.action, review.confidence_score, review.model
        ),
        None => warn!("⚠️ [AI Risk Officer] Phản hồi AI không hợp lệ — bỏ qua review."),
    }
    review
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EntryType, SignalStatus, SignalType, Timeframe};
    use chrono::TimeZone;

    fn sample_signal() -> Signal {
        Signal {
            id: "SIG-AI-REVIEW".to_string(),
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
            risk_reward_ratio: 2.0,
            volume_surge_ratio: 1.85,
            recommended_lot_size: 0.09,
            risk_amount_usd: 94.50,
            potential_profit_tp1_usd: 189.00,
            potential_profit_tp2_usd: 283.50,
            price_action_notes: "Bullish Pinbar tại hỗ trợ".to_string(),
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
            live_market_price: Some(2684.80),
            retest_limit_price: None,
            data_source: "BINANCE_PAXG".to_string(),
            is_entry_filled: true,
            ai_verdict: None,
            ai_confidence: None,
            ai_reason: None,
        }
    }

    fn review(action: &str, confidence: f64) -> AiSignalReview {
        AiSignalReview {
            action: action.to_string(),
            confidence_score: confidence,
            entry_zone: None,
            dynamic_sl: None,
            tp: None,
            macro_trend_h1_h4: None,
            reason: "structure agrees".to_string(),
            provider: "opencode_go".to_string(),
            model: DEFAULT_REVIEW_MODEL.to_string(),
        }
    }

    #[test]
    fn parses_valid_json_and_clamps_confidence() {
        let content = r#"{
            "action": "buy",
            "confidence_score": 1.4,
            "entry_zone": " 2680-2685 ",
            "dynamic_sl": 2672.5,
            "tp": null,
            "macro_trend_h1_h4": "H4 bullish",
            "reason": "  D1/H4 cùng hướng tăng.  "
        }"#;

        let parsed = parse_review_content(content, "opencode_go", DEFAULT_REVIEW_MODEL)
            .expect("valid review JSON");

        assert_eq!(parsed.action, "BUY");
        assert_eq!(parsed.confidence_score, 1.0);
        assert_eq!(parsed.entry_zone.as_deref(), Some("2680-2685"));
        assert_eq!(parsed.dynamic_sl, Some(2672.5));
        assert_eq!(parsed.tp, None);
        assert_eq!(parsed.macro_trend_h1_h4.as_deref(), Some("H4 bullish"));
        assert_eq!(parsed.reason, "D1/H4 cùng hướng tăng.");
        assert_eq!(parsed.provider, "opencode_go");
        assert_eq!(parsed.model, DEFAULT_REVIEW_MODEL);
    }

    #[test]
    fn parses_markdown_fenced_json_and_standby() {
        let content = "```json\n{\"action\":\"standby\",\"confidence_score\":0.6,\"reason\":\"red news\"}\n```";
        let parsed =
            parse_review_content(content, "opencode_go", "test-model").expect("fenced review JSON");
        assert_eq!(parsed.action, "STANDBY");
        assert_eq!(parsed.confidence_score, 0.6);
    }

    #[test]
    fn rejects_malformed_or_incomplete_reviews() {
        for content in [
            "not json",
            "{\"action\":\"HOLD\",\"confidence_score\":0.9,\"reason\":\"x\"}",
            "{\"action\":\"BUY\",\"confidence_score\":0.9,\"reason\":\"  \"}",
            "{\"action\":\"BUY\",\"reason\":\"missing confidence\"}",
            "{\"confidence_score\":0.9,\"reason\":\"missing action\"}",
            "{\"action\":\"BUY\",\"confidence_score\":\"high\",\"reason\":\"bad type\"}",
        ] {
            assert!(
                parse_review_content(content, "opencode_go", "test-model").is_none(),
                "expected None for {content}"
            );
        }
    }

    #[test]
    fn blocking_rules_cover_standby_opposite_and_low_confidence() {
        let candidate = sample_signal();

        let (blocked, reason) = review_blocks_signal(&candidate, &review("STANDBY", 0.99), 0.75);
        assert!(blocked);
        assert!(reason.contains("STANDBY"));

        let (blocked, reason) = review_blocks_signal(&candidate, &review("SELL", 0.95), 0.75);
        assert!(blocked);
        assert!(reason.contains("phản đối"));

        let (blocked, reason) = review_blocks_signal(&candidate, &review("BUY", 0.60), 0.75);
        assert!(blocked);
        assert!(reason.contains("thấp hơn ngưỡng"));

        let (blocked, reason) = review_blocks_signal(&candidate, &review("BUY", 0.80), 0.75);
        assert!(!blocked);
        assert!(reason.contains("đồng ý"));
    }

    #[test]
    fn blocking_matches_case_insensitively_and_clamps_threshold() {
        let candidate = sample_signal();
        let (blocked, _) = review_blocks_signal(&candidate, &review("buy", 0.90), 0.75);
        assert!(!blocked);

        let (blocked, _) = review_blocks_signal(&candidate, &review("BUY", 0.55), 0.1);
        assert!(!blocked, "threshold must clamp to 0.5, allowing 0.55");
    }

    #[test]
    fn blocking_rejects_non_finite_confidence() {
        let candidate = sample_signal();
        let (blocked, reason) = review_blocks_signal(&candidate, &review("BUY", f64::NAN), 0.75);
        assert!(blocked);
        assert!(reason.contains("không hợp lệ"));
    }

    #[test]
    fn prompt_contains_market_context_candidate_and_events() {
        let now = Utc
            .with_ymd_and_hms(2026, 11, 10, 14, 30, 0)
            .single()
            .expect("valid timestamp");
        let events = vec![EconomicEvent {
            id: "EVT-CPI".to_string(),
            title: "US Core CPI m/m".to_string(),
            currency: "USD".to_string(),
            impact: ImpactLevel::High,
            scheduled_time: now + Duration::hours(3),
            forecast: Some("0.3%".to_string()),
            previous: Some("0.2%".to_string()),
            actual: None,
        }];

        let prompt = build_review_prompt(&sample_signal(), None, &events, now);

        assert!(prompt.contains("side=BUY"));
        assert!(prompt.contains("Upcoming High-Impact USD events: US Core CPI m/m"));
        assert!(prompt.contains("IN golden session 13:00-23:00 VN"));
        assert!(prompt.contains("\"action\":\"BUY\"|"));
        assert!(prompt.contains("VETO-ONLY"));
    }

    #[test]
    fn session_id_uses_utc_date() {
        let now = Utc
            .with_ymd_and_hms(2026, 11, 10, 23, 30, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(review_session_id(now), "xauusd-review-2026-11-10");
    }

    #[test]
    fn review_settings_use_defaults_and_require_key() {
        assert!(resolve_review_settings("", "", "").is_none());
        assert!(resolve_review_settings("   ", "x", "y").is_none());

        let settings = resolve_review_settings(" key ", "", "").expect("settings");
        assert_eq!(settings.api_key, "key");
        assert_eq!(settings.api_base, OPENCODE_GO_API_BASE);
        assert_eq!(settings.model, DEFAULT_REVIEW_MODEL);

        let custom = resolve_review_settings("key", " https://example.test/v1 ", " glm-5.3-flash ")
            .expect("custom settings");
        assert_eq!(custom.api_base, "https://example.test/v1");
        assert_eq!(custom.model, "glm-5.3-flash");
    }

    #[tokio::test]
    async fn review_signal_returns_none_when_disabled_without_network() {
        let candidate = sample_signal();
        let config = BotConfig::default();
        assert!(!config.ai_signal_review_enabled);

        let result = review_signal(&candidate, None, &config, &[]).await;
        assert!(result.is_none());
    }
}
