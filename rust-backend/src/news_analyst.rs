//! AI News Impact Analyst (advisory, post-actual only).
//!
//! The analyst is deliberately narrow: it may classify an event and explain a
//! likely gold lean **after** `actual` is known. It never invents `actual`,
//! never returns Entry/SL/TP, and never approves/blocks a trade signal.
//!
//! Cost controls live in the worker (High USD + High relevance + actual only,
//! daily cap, two attempts per event). This module only builds/parses the call.

use crate::news_impact::{
    EventClass, GoldRelevance, HawkishDovish, Lean, NewsAiView, NewsImpactRecord,
};
use crate::signal_review::{
    normalize_json_content, post_ai_chat_completion, resolve_review_settings, review_session_id,
    truncate_chars,
};
use chrono::Utc;
use serde_json::Value;
use tracing::{info, warn};

pub const NEWS_ANALYST_PROMPT_VERSION: &str = "news_impact_v1";

const NEWS_ANALYST_SYSTEM_PROMPT: &str = "\
Bạn là News Impact Analyst cho XAU/USD. Bạn KHÔNG phải Risk Officer và KHÔNG tạo Entry/SL/TP. \
Nhiệm vụ: đánh giá tác động của một sự kiện kinh tế đã có actual lên vàng, ở mức advisory. \
\n\
NGUYÊN TẮC BẮT BUỘC:\n\
1. Không bịa actual/forecast. Chỉ dùng dữ liệu được cung cấp.\n\
2. gold_lean chỉ được là UP, DOWN hoặc UNCLEAR. Không được đưa ra Entry/SL/TP/Lot.\n\
3. Với CPI/PCE/PPI/NFP/GDP/Retail Sales/PMI: actual > forecast thường là hawkish cho USD, \
vàng nghiêng DOWN; actual < forecast là dovish, vàng nghiêng UP. Với Unemployment Rate/Jobless Claims: \
actual > forecast là lao động yếu, vàng nghiêng UP; actual < forecast là vàng nghiêng DOWN.\n\
4. Với Fed speech/minutes: nếu không có nội dung phát biểu, gold_lean phải là UNCLEAR và ghi rõ điều kiện.\n\
5. Nếu dữ liệu mâu thuẫn hoặc không đủ, trả UNCLEAR thay vì đoán. confidence <= 40 khi thiếu actual hoặc thiếu ngữ cảnh.\n\
6. reasoning ngắn gọn, <= 240 ký tự, không markdown.\n\
\n\
TRẢ VỀ JSON THUẦN (không markdown, không giải thích ngoài JSON):\n\
{\"event_class\":\"INFLATION|LABOR|GROWTH|CONSUMPTION|HOUSING|MANUFACTURING|SENTIMENT|FED|GEOPOLITICAL|OTHER\",\
\"gold_relevance\":\"HIGH|MEDIUM|LOW\",\"hawkish_dovish\":\"HAWKISH|DOVISH|NEUTRAL|UNKNOWN\",\
\"gold_lean\":\"UP|DOWN|UNCLEAR\",\"confidence\":0,\"conditions\":\"short text or null\",\
\"reasoning\":\"non-empty, <=240 chars\"}";

fn format_optional(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("n/a")
        .to_string()
}

fn format_points(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:+.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

/// Build the user brief. Pure and testable.
pub(crate) fn build_news_analyst_prompt(record: &NewsImpactRecord) -> String {
    format!(
        "Event: {}\n\
         Impact: {:?} · Currency: {}\n\
         Scheduled: {} UTC\n\
         Forecast: {} · Previous: {} · Actual: {}\n\
         Deterministic class: {} · Gold relevance: {}\n\
         Gold reaction so far: +15m {} pts · +60m {} pts · +240m {} pts · reaction {}\n\
         Volume ratio: {}\n\
         Price source: {} · Volume source: {}\n\
         \n\
         Hãy đánh giá tác động lên vàng. Nếu actual có, so sánh actual với forecast và giải thích \
         kênh truyền dẫn (USD/lợi suất thực). Nếu không đủ dữ liệu, trả UNCLEAR.",
        record.title,
        record.impact,
        record.currency,
        record.scheduled_time.format("%Y-%m-%d %H:%M"),
        format_optional(record.forecast.as_deref()),
        format_optional(record.previous.as_deref()),
        format_optional(record.actual.as_deref()),
        record.event_class.as_str(),
        record.gold_relevance.as_str(),
        format_points(record.return_15m),
        format_points(record.return_60m),
        format_points(record.return_240m),
        record.reaction.as_str(),
        record
            .volume_ratio
            .filter(|value| value.is_finite())
            .map(|value| format!("{value:.2}×"))
            .unwrap_or_else(|| "n/a".to_string()),
        record.price_source,
        record.volume_source,
    )
}

fn parse_event_class(value: &str) -> Option<EventClass> {
    match value.trim().to_uppercase().as_str() {
        "INFLATION" => Some(EventClass::Inflation),
        "LABOR" => Some(EventClass::Labor),
        "GROWTH" => Some(EventClass::Growth),
        "CONSUMPTION" => Some(EventClass::Consumption),
        "HOUSING" => Some(EventClass::Housing),
        "MANUFACTURING" => Some(EventClass::Manufacturing),
        "SENTIMENT" => Some(EventClass::Sentiment),
        "FED" => Some(EventClass::Fed),
        "GEOPOLITICAL" => Some(EventClass::Geopolitical),
        "OTHER" => Some(EventClass::Other),
        _ => None,
    }
}

fn parse_gold_relevance(value: &str) -> Option<GoldRelevance> {
    match value.trim().to_uppercase().as_str() {
        "HIGH" => Some(GoldRelevance::High),
        "MEDIUM" => Some(GoldRelevance::Medium),
        "LOW" => Some(GoldRelevance::Low),
        _ => None,
    }
}

fn parse_hawkish_dovish(value: &str) -> Option<HawkishDovish> {
    match value.trim().to_uppercase().as_str() {
        "HAWKISH" => Some(HawkishDovish::Hawkish),
        "DOVISH" => Some(HawkishDovish::Dovish),
        "NEUTRAL" => Some(HawkishDovish::Neutral),
        "UNKNOWN" => Some(HawkishDovish::Unknown),
        _ => None,
    }
}

fn parse_gold_lean(value: &str) -> Option<Lean> {
    match value.trim().to_uppercase().as_str() {
        "UP" => Some(Lean::Up),
        "DOWN" => Some(Lean::Down),
        "UNCLEAR" => Some(Lean::Unclear),
        _ => None,
    }
}

/// Parse + validate the analyst JSON. Malformed/invalid enums => None.
pub(crate) fn parse_news_ai_content(content: &str, model: &str) -> Option<NewsAiView> {
    let value: Value = serde_json::from_str(normalize_json_content(content)).ok()?;
    let event_class = parse_event_class(value.get("event_class")?.as_str()?)?;
    let gold_relevance = parse_gold_relevance(value.get("gold_relevance")?.as_str()?)?;
    let hawkish_dovish = parse_hawkish_dovish(value.get("hawkish_dovish")?.as_str()?)?;
    let gold_lean = parse_gold_lean(value.get("gold_lean")?.as_str()?)?;
    let confidence = value
        .get("confidence")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())?
        .clamp(0.0, 100.0) as u8;
    let reasoning = value
        .get("reasoning")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())?;
    let conditions = value
        .get("conditions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| truncate_chars(text, 240));

    Some(NewsAiView {
        generated_at: Utc::now(),
        model: model.to_string(),
        prompt_version: NEWS_ANALYST_PROMPT_VERSION.to_string(),
        event_class,
        gold_relevance,
        hawkish_dovish,
        gold_lean,
        confidence,
        conditions,
        reasoning: truncate_chars(reasoning, 240),
    })
}

/// Call the AI analyst for one record. Caller owns eligibility/cost gating.
pub async fn analyze_news_impact(record: &NewsImpactRecord) -> Option<NewsAiView> {
    record.actual.as_ref()?;
    let settings = resolve_review_settings(
        &std::env::var("OPENCODE_GO_API_KEY").unwrap_or_default(),
        &std::env::var("OPENCODE_GO_BASE_URL").unwrap_or_default(),
        &std::env::var("AI_SIGNAL_REVIEW_MODEL").unwrap_or_default(),
    )?;

    let prompt = build_news_analyst_prompt(record);
    let (content, model_used) = post_ai_chat_completion(
        &settings,
        NEWS_ANALYST_SYSTEM_PROMPT,
        &prompt,
        &review_session_id(Utc::now()),
        0.2,
        "News Impact Analyst",
    )
    .await?;

    match parse_news_ai_content(&content, &model_used) {
        Some(view) => {
            info!(
                "📰 [News Impact] {} → {} {}% (model={})",
                record.event_id,
                view.gold_lean.as_str(),
                view.confidence,
                view.model
            );
            Some(view)
        }
        None => {
            warn!(
                "⚠️ [News Impact] Phản hồi AI không hợp lệ cho {} — bỏ qua. Snippet: {}",
                record.event_id,
                truncate_chars(&content, 200)
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::news_filter::ImpactLevel;
    use chrono::TimeZone;

    fn record() -> NewsImpactRecord {
        NewsImpactRecord {
            event_id: "FF-1".to_string(),
            title: "Core CPI m/m".to_string(),
            impact: ImpactLevel::High,
            scheduled_time: Utc
                .with_ymd_and_hms(2026, 11, 17, 13, 30, 0)
                .single()
                .expect("time"),
            event_class: EventClass::Inflation,
            gold_relevance: GoldRelevance::High,
            forecast: Some("0.3%".to_string()),
            actual: Some("0.5%".to_string()),
            conditional_lean: Lean::Down,
            conditional_reason: "Actual 0.50 vs forecast 0.30".to_string(),
            ..NewsImpactRecord::default()
        }
    }

    #[test]
    fn prompt_contains_event_and_actual() {
        let prompt = build_news_analyst_prompt(&record());
        assert!(prompt.contains("Core CPI m/m"));
        assert!(prompt.contains("Actual: 0.5%"));
        assert!(prompt.contains("Deterministic class: INFLATION"));
        assert!(!prompt.contains("Deterministic conditional lean"));
    }

    #[test]
    fn parses_valid_json_with_markdown_fence() {
        let content = r#"```json
        {"event_class":"INFLATION","gold_relevance":"HIGH","hawkish_dovish":"HAWKISH",
         "gold_lean":"DOWN","confidence":72,"conditions":"if core services stay hot",
         "reasoning":"Actual above forecast supports USD and real yields."}
        ```"#;
        let view = parse_news_ai_content(content, "test-model").expect("valid");
        assert_eq!(view.event_class, EventClass::Inflation);
        assert_eq!(view.gold_lean, Lean::Down);
        assert_eq!(view.confidence, 72);
        assert_eq!(view.model, "test-model");
    }

    #[test]
    fn rejects_invalid_lean() {
        let content = r#"{"event_class":"INFLATION","gold_relevance":"HIGH","hawkish_dovish":"HAWKISH",
            "gold_lean":"MAYBE","confidence":50,"reasoning":"x"}"#;
        assert!(parse_news_ai_content(content, "m").is_none());
    }

    #[test]
    fn rejects_missing_reasoning() {
        let content = r#"{"event_class":"INFLATION","gold_relevance":"HIGH","hawkish_dovish":"HAWKISH",
            "gold_lean":"DOWN","confidence":50,"reasoning":" "}"#;
        assert!(parse_news_ai_content(content, "m").is_none());
    }

    #[test]
    fn clamps_confidence() {
        let content = r#"{"event_class":"OTHER","gold_relevance":"LOW","hawkish_dovish":"UNKNOWN",
            "gold_lean":"UNCLEAR","confidence":999,"reasoning":"not enough data"}"#;
        let view = parse_news_ai_content(content, "m").expect("valid");
        assert_eq!(view.confidence, 100);
    }
}
