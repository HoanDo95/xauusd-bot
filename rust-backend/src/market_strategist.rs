//! AI Market Strategist (Phase 2) — H1 narrative + scenario selection.
//!
//! This is deliberately **advisory**. It never approves a sniper signal and
//! never invents Entry/SL/TP: the AI may only select a `scenario_id` from the
//! deterministic scenario builder, and the code keeps the levels. The Risk
//! Officer / AI Decision prompt remains separate and unchanged.

use crate::market_context::MarketContext;
use crate::market_view::MarketScore;
use crate::models::Candle;
use crate::scenarios::{is_reference_only_scenario, Scenario, ScenarioResponse};
use crate::signal_review::{
    normalize_json_content, post_ai_chat_completion, resolve_review_settings, review_session_id,
    truncate_chars, truncate_sentences,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

pub const STRATEGIST_PROMPT_VERSION: &str = "strategist_v1";

/// AI narrative + selected scenario ids. Levels are resolved from the
/// deterministic `ScenarioResponse` by the caller, never from the AI.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StrategistView {
    pub generated_at: String,
    pub model: String,
    pub prompt_version: String,
    pub market_view: String,
    pub structure: String,
    pub primary_scenario_id: Option<String>,
    pub primary_action: Option<String>,
    pub alternative_scenario_id: Option<String>,
    pub risk_notes: Vec<String>,
}

const STRATEGIST_SYSTEM_PROMPT: &str = "\
Bạn là Senior XAU/USD Strategist (quantitative + price action), không phải Risk Officer duyệt lệnh. \
Nhiệm vụ của bạn là đọc dữ liệu định lượng được cung cấp để đưa ra MARKET VIEW và 2 kịch bản hành động. \
Bạn KHÔNG được bịa Entry/SL/TP và KHÔNG được tự tạo scenario mới.\n\
\n\
NGUYÊN TẮC BẮT BUỘC:\n\
1. Tư duy xác suất, không tuyệt đối. Trọng số: cấu trúc H4/H1 + key level 60%; RSI/volume M15 30%; macro 10%.\n\
2. Luôn cung cấp vùng chờ. Nếu giá đang lơ lửng, dùng action WATCH_SELL / WATCH_BUY / STAND_ASIDE nhưng vẫn phải chỉ rõ vùng quan sát.\n\
3. Phân biệt Technical Pullback và Reversal: nếu H4/H1 giảm mà M15 tăng mạnh, gọi là pullback lên kháng cự để canh Sell, trừ khi H1 đóng cửa vượt invalidation. Mirror cho xu hướng tăng.\n\
4. Chỉ cảnh báo NO_TRADE cứng khi có tin đỏ High Impact trong ±30 phút. Các yếu tố khác chỉ dùng để điều chỉnh Entry/SL/độ tin cậy.\n\
5. KHÔNG bịa giá. Chỉ chọn primary_scenario_id / alternative_scenario_id từ danh sách scenario được cung cấp. Levels do code giữ.\n\
6. Nếu dữ liệu thiếu/mâu thuẫn, ghi rõ trong risk_notes; vẫn giữ Market View thay vì im lặng.\n\
7. CHỈ chọn scenario có valid=true. Scenario valid=false (ví dụ bearish_rejection đang tắt vì backtest âm) chỉ để tham khảo, KHÔNG được chọn làm PRIMARY/ALTERNATIVE.\n\
8. `bullish_bounce` là REFERENCE_ONLY (backtest 2022–2026 âm). KHÔNG được chọn làm PRIMARY/ALTERNATIVE; chỉ có thể nhắc trong market_view/risk_notes.\n\
9. Viết cực gọn: market_view tối đa 3 câu / 120 từ; risk_notes tối đa 3 ý, mỗi ý dưới 20 từ. Không lặp lại số liệu đã có trong brief.\n\
\n\
TRẢ VỀ JSON THUẦN (không markdown, không giải thích ngoài JSON):\n\
{\"market_view\":\"2-3 câu\",\"structure\":\"TREND_BULL|TREND_BEAR|TECHNICAL_PULLBACK|RANGE|REVERSAL_WATCH|MIXED\",\"primary_scenario_id\":\"id hoặc null\",\"primary_action\":\"WATCH_SELL|WATCH_BUY|STAND_ASIDE\",\"alternative_scenario_id\":\"id hoặc null\",\"risk_notes\":[\"tối đa 3 ghi chú ngắn\"]}";

/// Build the strategist user brief. Pure and testable.
#[allow(clippy::too_many_arguments)]
pub fn build_strategist_prompt(
    scenarios: &ScenarioResponse,
    context: Option<&MarketContext>,
    news_notes: &[String],
    no_trade_reason: Option<&str>,
    session_in: bool,
    volume_source: &str,
    volume_ratio: Option<f64>,
    score: Option<&MarketScore>,
) -> String {
    let price = scenarios
        .price
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string());
    let (d1, h4, h1, h4_atr, support, resistance) = match context {
        Some(context) => (
            context.d1.bias.as_str(),
            context.h4.bias.as_str(),
            context.h1.bias.as_str(),
            format!("{:.2}", context.h4.atr14),
            context
                .h4
                .nearest_support
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "n/a".to_string()),
            context
                .h4
                .nearest_resistance
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "n/a".to_string()),
        ),
        None => (
            "UNKNOWN",
            "UNKNOWN",
            "UNKNOWN",
            "n/a".to_string(),
            "n/a".to_string(),
            "n/a".to_string(),
        ),
    };
    let rsi_m15 = context
        .and_then(|context| context.rsi_m15.as_ref())
        .and_then(|rsi| rsi.value)
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "n/a".to_string());
    let rsi_h1 = context
        .and_then(|context| context.rsi_h1.as_ref())
        .and_then(|rsi| rsi.value)
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "n/a".to_string());

    let scenario_lines = if scenarios.scenarios.is_empty() {
        "- (no scenarios)".to_string()
    } else {
        scenarios
            .scenarios
            .iter()
            .map(format_scenario_line)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let news = if news_notes.is_empty() {
        "none".to_string()
    } else {
        news_notes.join(" | ")
    };
    let volume_text = volume_ratio
        .map(|ratio| format!("{ratio:.2}× MA20"))
        .unwrap_or_else(|| "n/a".to_string());
    let score_text = score
        .map(|score| {
            format!(
                "structure {}/60 · trigger {}/30 · macro {}/10 · total {}/100",
                score.structure, score.trigger, score.macro_score, score.total
            )
        })
        .unwrap_or_else(|| "n/a".to_string());

    format!(
        "Current price: {price}\n\
         D1/H4/H1: {d1}/{h4}/{h1} · H4 ATR: {h4_atr} · H4 support: {support} · H4 resistance: {resistance}\n\
         RSI M15: {rsi_m15} · RSI H1: {rsi_h1}\n\
         Volume M15: {volume_text} · source={volume_source}\n\
         Deterministic 60/30/10 score: {score_text}\n\
         Session 13:00-23:00 VN: {}\n\
         NO_TRADE hard reason: {}\n\
         Upcoming red news: {news}\n\
         \n\
         Deterministic scenarios (choose ids from this list only):\n{scenario_lines}\n\
         \n\
         Hãy viết Market View ngắn gọn và chọn PRIMARY/ALTERNATIVE scenario id phù hợp. \
         Không bịa Entry/SL/TP; levels sẽ lấy từ scenario đã chọn.",
        if session_in { "IN" } else { "OUT" },
        no_trade_reason.unwrap_or("none"),
    )
}

fn format_scenario_line(scenario: &Scenario) -> String {
    format!(
        "- id={} | role={} | bias={} | valid={} | reference_only={} | confidence={} | entry={} | SL={} | TP1={} | TP2={} | trigger={} | invalidation={}",
        scenario.id,
        scenario.role,
        scenario.bias,
        scenario.valid,
        is_reference_only_scenario(&scenario.id),
        scenario.confidence,
        format_optional(scenario.entry),
        format_optional(scenario.stop_loss),
        format_optional(scenario.take_profit_1),
        format_optional(scenario.take_profit_2),
        truncate_chars(&scenario.trigger, 120),
        truncate_chars(&scenario.invalidation, 120),
    )
}

fn format_optional(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

/// Latest M15 volume divided by the average of the previous `period` candles.
pub fn volume_surge_ratio(candles: &[Candle], period: usize) -> Option<f64> {
    let period = period.max(1);
    if candles.len() <= period {
        return None;
    }
    let last = candles.last()?.volume;
    let window = &candles[candles.len() - 1 - period..candles.len() - 1];
    let average = window.iter().map(|candle| candle.volume).sum::<f64>() / period as f64;
    if !average.is_finite() || average <= 0.0 {
        return None;
    }
    Some((last / average).max(0.0))
}

/// Parse and validate the strategist JSON. Unknown scenario ids are rejected
/// and replaced with the deterministic PRIMARY/ALTERNATE ids.
pub fn parse_strategist_content(
    content: &str,
    scenarios: &ScenarioResponse,
    model: &str,
) -> Option<StrategistView> {
    let value: Value = serde_json::from_str(normalize_json_content(content)).ok()?;
    let market_view = value
        .get("market_view")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let market_view = truncate_sentences(market_view, 1500);
    let structure = value
        .get("structure")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| truncate_chars(value, 40))
        .unwrap_or_else(|| "MIXED".to_string());

    let primary_id = valid_scenario_id(
        scenarios,
        value
            .get("primary_scenario_id")
            .and_then(|value| value.as_str()),
    )
    .or_else(|| {
        scenarios
            .scenarios
            .iter()
            .find(|scenario| {
                scenario.role.eq_ignore_ascii_case("PRIMARY")
                    && !is_reference_only_scenario(&scenario.id)
            })
            .map(|scenario| scenario.id.clone())
    })
    .or_else(|| {
        scenarios
            .scenarios
            .iter()
            .find(|scenario| !is_reference_only_scenario(&scenario.id))
            .map(|scenario| scenario.id.clone())
    });
    let alternative_id = valid_scenario_id(
        scenarios,
        value
            .get("alternative_scenario_id")
            .and_then(|value| value.as_str()),
    )
    .filter(|id| Some(id) != primary_id.as_ref())
    .or_else(|| {
        scenarios
            .scenarios
            .iter()
            .find(|scenario| {
                scenario.role.eq_ignore_ascii_case("ALTERNATE")
                    && scenario.valid
                    && !is_reference_only_scenario(&scenario.id)
                    && Some(&scenario.id) != primary_id.as_ref()
            })
            .map(|scenario| scenario.id.clone())
    });

    let ai_action = value
        .get("primary_action")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_uppercase())
        .filter(|value| matches!(value.as_str(), "WATCH_SELL" | "WATCH_BUY" | "STAND_ASIDE"));
    // The deterministic scenario bias is the source of truth. The AI action is
    // only a fallback when no scenario was selected.
    let primary_action = primary_id
        .as_ref()
        .and_then(|id| {
            scenarios
                .scenarios
                .iter()
                .find(|scenario| &scenario.id == id)
        })
        .map(
            |scenario| match scenario.bias.trim().to_uppercase().as_str() {
                "BEARISH" => "WATCH_SELL".to_string(),
                "BULLISH" => "WATCH_BUY".to_string(),
                _ => "STAND_ASIDE".to_string(),
            },
        )
        .or(ai_action);

    let risk_notes = value
        .get("risk_notes")
        .and_then(|value| value.as_array())
        .map(|notes| {
            notes
                .iter()
                .filter_map(|note| note.as_str())
                .map(|note| truncate_chars(note.trim(), 200))
                .filter(|note| !note.is_empty())
                .take(3)
                .collect()
        })
        .unwrap_or_default();

    let view = StrategistView {
        generated_at: Utc::now().to_rfc3339(),
        model: model.to_string(),
        prompt_version: STRATEGIST_PROMPT_VERSION.to_string(),
        market_view,
        structure,
        primary_scenario_id: primary_id,
        primary_action,
        alternative_scenario_id: alternative_id,
        risk_notes,
    };
    info!(
        "🧠 [Market Strategist] model={} structure={} primary={} action={}",
        view.model,
        view.structure,
        view.primary_scenario_id.as_deref().unwrap_or("none"),
        view.primary_action.as_deref().unwrap_or("none")
    );
    Some(view)
}

fn valid_scenario_id(scenarios: &ScenarioResponse, candidate: Option<&str>) -> Option<String> {
    let candidate = candidate?.trim();
    if candidate.is_empty() {
        return None;
    }
    scenarios
        .scenarios
        .iter()
        .find(|scenario| {
            scenario.id == candidate && scenario.valid && !is_reference_only_scenario(&scenario.id)
        })
        .map(|scenario| scenario.id.clone())
}

/// Call the AI strategist. Returns None on any provider/parse failure so the
/// deterministic Market View remains available.
#[allow(clippy::too_many_arguments)]
pub async fn review_market_strategist(
    scenarios: &ScenarioResponse,
    context: Option<&MarketContext>,
    news_notes: &[String],
    no_trade_reason: Option<&str>,
    session_in: bool,
    volume_source: &str,
    volume_ratio: Option<f64>,
    score: Option<&MarketScore>,
) -> Option<StrategistView> {
    let settings = resolve_review_settings(
        &std::env::var("OPENCODE_GO_API_KEY").unwrap_or_default(),
        &std::env::var("OPENCODE_GO_BASE_URL").unwrap_or_default(),
        &std::env::var("AI_SIGNAL_REVIEW_MODEL").unwrap_or_default(),
    )?;
    let prompt = build_strategist_prompt(
        scenarios,
        context,
        news_notes,
        no_trade_reason,
        session_in,
        volume_source,
        volume_ratio,
        score,
    );
    let (content, model_used) = post_ai_chat_completion(
        &settings,
        STRATEGIST_SYSTEM_PROMPT,
        &prompt,
        &review_session_id(Utc::now()),
        0.3,
        "Market Strategist",
    )
    .await?;
    let view = parse_strategist_content(&content, scenarios, &model_used);
    if view.is_none() {
        warn!("⚠️ [Market Strategist] Phản hồi không hợp lệ — dùng deterministic view.");
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use crate::scenarios::ScenarioMarket;

    fn candle(volume: f64) -> Candle {
        Candle {
            timestamp: 0,
            open: 1.0,
            high: 1.0,
            low: 1.0,
            close: 1.0,
            volume,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    fn scenario(id: &str, role: &str, bias: &str, entry: f64) -> Scenario {
        Scenario {
            id: id.to_string(),
            name: id.to_string(),
            bias: bias.to_string(),
            trigger: "test trigger".to_string(),
            condition: "test".to_string(),
            entry: Some(entry),
            stop_loss: Some(entry + 10.0),
            take_profit_1: Some(entry - 20.0),
            take_profit_2: Some(entry - 30.0),
            risk_reward: Some(2.0),
            trigger_price: Some(entry),
            trigger_direction: "BELOW".to_string(),
            trigger_timeframe: "M15".to_string(),
            invalidation: "test invalidation".to_string(),
            confidence: 80,
            macro_alignment: "ALIGNED".to_string(),
            valid: true,
            role: role.to_string(),
            priority: 80,
            notes: vec![],
        }
    }

    fn response() -> ScenarioResponse {
        ScenarioResponse {
            status: "ok".to_string(),
            generated_at: "2026-11-17T00:00:00Z".to_string(),
            price: Some(2700.0),
            source: "TEST".to_string(),
            volume_source: "NONE".to_string(),
            market: Some(ScenarioMarket {
                h4_bias: "BEARISH".to_string(),
                h1_bias: "BEARISH".to_string(),
                d1_bias: "BEARISH".to_string(),
                h4_atr: 12.0,
                nearest_support: Some(2690.0),
                nearest_resistance: Some(2710.0),
                rsi_m15: Some(50.0),
                rsi_state: "Neutral".to_string(),
                macro_bias: "BEARISH".to_string(),
                macro_confidence: 80,
                daily_change_pct: Some(-0.3),
                rsi_h1: None,
                rsi_h4: None,
                rsi_d1: None,
            }),
            scenarios: vec![
                scenario("primary", "PRIMARY", "BEARISH", 2700.0),
                scenario("alt", "ALTERNATE", "BULLISH", 2710.0),
            ],
            warnings: vec![],
        }
    }

    #[test]
    fn volume_surge_ratio_uses_previous_average() {
        let candles = vec![candle(100.0), candle(100.0), candle(200.0)];
        assert_eq!(volume_surge_ratio(&candles, 2), Some(2.0));
        assert!(volume_surge_ratio(&candles, 3).is_none());
    }

    #[test]
    fn prompt_contains_scenarios_weights_and_json_schema() {
        let prompt = build_strategist_prompt(
            &response(),
            None,
            &["Tin đỏ USD 21:30".to_string()],
            None,
            true,
            "TEST_PROXY",
            Some(1.2),
            None,
        );
        assert!(prompt.contains("id=primary"));
        assert!(prompt.contains("id=alt"));
        assert!(prompt.contains("Volume M15: 1.20× MA20"));
        assert!(prompt.contains("source=TEST_PROXY"));
        assert!(prompt.contains("Tin đỏ USD 21:30"));
        assert!(prompt.contains("Current price: 2700.00"));
        assert!(prompt.contains("Session 13:00-23:00 VN: IN"));
    }

    #[test]
    fn parses_valid_json_and_keeps_deterministic_ids() {
        let view = parse_strategist_content(
            r#"{"market_view":"H4 bearish, cho Sell tại kháng cự.","structure":"TECHNICAL_PULLBACK","primary_scenario_id":"primary","primary_action":"WATCH_SELL","alternative_scenario_id":"alt","risk_notes":["Tin đỏ 21:30"]}"#,
            &response(),
            "test-model",
        )
        .expect("valid view");
        assert_eq!(view.primary_scenario_id.as_deref(), Some("primary"));
        assert_eq!(view.alternative_scenario_id.as_deref(), Some("alt"));
        assert_eq!(view.primary_action.as_deref(), Some("WATCH_SELL"));
        assert_eq!(view.prompt_version, STRATEGIST_PROMPT_VERSION);
        assert_eq!(view.risk_notes.len(), 1);
    }

    #[test]
    fn unknown_scenario_id_falls_back_to_deterministic_roles() {
        let view = parse_strategist_content(
            r#"{"market_view":"View.","structure":"MIXED","primary_scenario_id":"invented","primary_action":"WATCH_BUY","alternative_scenario_id":"also-invented","risk_notes":[]}"#,
            &response(),
            "test-model",
        )
        .expect("valid view");
        assert_eq!(view.primary_scenario_id.as_deref(), Some("primary"));
        assert_eq!(view.alternative_scenario_id.as_deref(), Some("alt"));
    }

    #[test]
    fn reference_only_bounce_is_rejected_as_primary() {
        let mut scenarios = response();
        scenarios
            .scenarios
            .push(scenario("bullish_bounce", "WATCHLIST", "BULLISH", 2695.0));

        let view = parse_strategist_content(
            r#"{"market_view":"Bounce reference.","structure":"MIXED","primary_scenario_id":"bullish_bounce","primary_action":"WATCH_BUY","alternative_scenario_id":"bullish_bounce","risk_notes":[]}"#,
            &scenarios,
            "test-model",
        )
        .expect("valid view");

        assert_eq!(view.primary_scenario_id.as_deref(), Some("primary"));
        assert_eq!(view.alternative_scenario_id.as_deref(), Some("alt"));
    }

    #[test]
    fn empty_market_view_is_rejected() {
        assert!(parse_strategist_content(
            r#"{"market_view":"","structure":"MIXED","primary_scenario_id":"primary","primary_action":"WATCH_SELL","risk_notes":[]}"#,
            &response(),
            "test-model"
        )
        .is_none());
    }
}
