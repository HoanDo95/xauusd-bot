//! AI Decision Mode: the AI becomes the primary BUY/SELL/STANDBY decision layer.
//!
//! Deterministic code still owns every hard safety rule: SL/TP ordering, R:R,
//! risk budget, daily quota, session window, news kill switch and dedup. The AI
//! only chooses a direction from the supplied candidate/scenarios (or STANDBY);
//! it never invents prices and never writes live SL/TP.

use crate::macro_flow::{align_signal_with_macro_flow, MacroFlowSnapshot};
use crate::market_context::MarketContext;
use crate::models::{BotConfig, EconomicEvent, ImpactLevel, Signal};
use crate::rsi::RsiSnapshot;
use crate::scenarios::ScenarioResponse;
use crate::signal_review::{
    format_metric_value, format_rsi_value, format_timeframe_context, normalize_json_content,
    normalize_macro_flow_alignment, optional_string, post_ai_chat_completion,
    resolve_review_settings, review_min_confidence, review_session_id, truncate_chars,
};
use crate::volume_delta::DeltaSnapshot;
use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

/// Primary AI verdict for XAU/USD. Never an automatic order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiDecision {
    /// BUY | SELL | STANDBY
    pub action: String,
    /// 0.0..=1.0
    pub confidence_score: f64,
    /// PRIMARY | ALTERNATE | WATCHLIST | NONE
    pub selected_scenario: Option<String>,
    pub reason: String,
    pub execution_rule: Option<String>,
    pub macro_flow_alignment: Option<String>,
    pub macro_flow_note: Option<String>,
    pub provider: String,
    pub model: String,
}

/// True only when the user enabled AI review AND selected DECISION mode.
pub fn ai_decision_enabled(config: &BotConfig) -> bool {
    config.ai_signal_review_enabled && config.ai_review_mode.eq_ignore_ascii_case("DECISION")
}

/// Clone config for candidate discovery in DECISION mode.
///
/// Volume and macro-bias are soft technical filters: disabling them lets the AI
/// review candidates that deterministic code would previously reject. Hard risk
/// rules (SL/TP, R:R, risk budget, quota, session, news, dedup) are unchanged.
pub fn relaxed_candidate_config(config: &BotConfig) -> BotConfig {
    let mut relaxed = config.clone();
    relaxed.volume_filter_enabled = false;
    relaxed.macro_bias = "NEUTRAL".to_string();
    relaxed
}

/// Char-safe truncation that keeps the result (including the ellipsis) within
/// `max_chars`, unlike `truncate_chars` which can exceed the cap by one.
fn truncate_inclusive(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        truncate_chars(value, max_chars.saturating_sub(1))
    }
}

fn format_optional_price(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_scenario_levels(scenario: &crate::scenarios::Scenario) -> String {
    format!(
        "entry={} SL={} TP1={} TP2={} R:R={}",
        format_optional_price(scenario.entry),
        format_optional_price(scenario.stop_loss),
        format_optional_price(scenario.take_profit_1),
        format_optional_price(scenario.take_profit_2),
        scenario
            .risk_reward
            .filter(|value| value.is_finite())
            .map(|value| format!("1:{value:.2}"))
            .unwrap_or_else(|| "n/a".to_string())
    )
}

fn format_scenario_block(scenarios: Option<&ScenarioResponse>) -> String {
    let Some(response) = scenarios else {
        return "Scenario view unavailable.".to_string();
    };
    if response.scenarios.is_empty() {
        return format!(
            "Scenario view status={} but no scenarios were built.",
            response.status
        );
    }

    response
        .scenarios
        .iter()
        .map(|scenario| {
            let note = scenario
                .notes
                .first()
                .map(|note| truncate_chars(note, 160))
                .unwrap_or_default();
            format!(
                "[{}] {} | bias={} | valid={} | confidence={} | macro={} | trigger={} | {} | invalidation={} | note={}",
                scenario.role,
                truncate_chars(&scenario.name, 80),
                scenario.bias,
                scenario.valid,
                scenario.confidence,
                scenario.macro_alignment,
                truncate_chars(&scenario.trigger, 180),
                format_scenario_levels(scenario),
                truncate_chars(&scenario.invalidation, 160),
                note
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_macro_metric(snapshot: &MacroFlowSnapshot, key: &str, decimals: usize) -> String {
    let metric = snapshot.metrics.iter().find(|metric| metric.key == key);
    let value = format_metric_value(metric.and_then(|metric| metric.value), decimals);
    let change_5d = metric
        .and_then(|metric| metric.change_5d_pct)
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:+.2}%"))
        .unwrap_or_else(|| "n/a".to_string());
    format!("{value} (5d {change_5d})")
}

fn format_macro_flow_block(
    candidate: Option<&Signal>,
    macro_flow: Option<&MacroFlowSnapshot>,
) -> String {
    let Some(snapshot) = macro_flow else {
        return "Macro & Flow: unavailable.".to_string();
    };

    let scorecard = &snapshot.scorecard;
    let candidate_side = candidate
        .map(|candidate| candidate.signal_type.as_str())
        .unwrap_or("");
    let alignment = align_signal_with_macro_flow(candidate_side, Some(snapshot));
    let cot_line = snapshot
        .cot
        .as_ref()
        .map(|cot| {
            format!(
                "COT net={:.0} (weekly change {:+.0}, {:.2}% OI)",
                cot.noncommercial_net, cot.noncommercial_net_change, cot.net_pct_oi
            )
        })
        .unwrap_or_else(|| "COT: n/a".to_string());

    format!(
        "Macro & Flow scorecard:\n\
        bias={} score={} confidence={} direction={} flow_quality={} driver={}\n\
        real_yield_10y={} DXY={} gold={} GLD={} IAU={}\n\
        {}\n\
        VIX={} GVZ={} MMF={}\n\
        risk_regime={} rate_regime={} usd_regime={} vol_regime={}\n\
        alignment_with_candidate={} note={}",
        scorecard.macro_bias,
        scorecard.score,
        scorecard.confidence,
        scorecard.direction,
        scorecard.flow_quality,
        truncate_chars(&scorecard.driver, 180),
        format_macro_metric(snapshot, "real_yield_10y", 2),
        format_macro_metric(snapshot, "dxy", 1),
        format_macro_metric(snapshot, "gold", 0),
        format_macro_metric(snapshot, "gld", 2),
        format_macro_metric(snapshot, "iau", 2),
        cot_line,
        format_macro_metric(snapshot, "vix", 2),
        format_macro_metric(snapshot, "gvz", 2),
        format_macro_metric(snapshot, "mmf", 0),
        scorecard.risk_regime,
        scorecard.rate_regime,
        scorecard.usd_regime,
        scorecard.vol_regime,
        alignment.alignment,
        truncate_chars(&alignment.note, 200)
    )
}

fn format_rsi_block(rsi: Option<&RsiSnapshot>, context: Option<&MarketContext>) -> String {
    let m15 = rsi
        .map(format_rsi_value)
        .or_else(|| {
            context
                .and_then(|context| context.rsi_m15.as_ref())
                .map(format_rsi_value)
        })
        .unwrap_or_else(|| "n/a".to_string());
    let h1 = context
        .and_then(|context| context.rsi_h1.as_ref())
        .map(format_rsi_value)
        .unwrap_or_else(|| "n/a".to_string());
    let h4 = context
        .and_then(|context| context.rsi_h4.as_ref())
        .map(format_rsi_value)
        .unwrap_or_else(|| "n/a".to_string());

    format!(
        "rsi_m15: {m15}\nrsi_h1: {h1}\nrsi_h4: {h4}\n\
        RSI semantics: 70/30 means >=70 Overbought and <=30 Oversold. \
        It flags an extended move, not an automatic reversal; divergence is a warning, not an entry."
    )
}

fn format_delta_block(delta: Option<&DeltaSnapshot>, volume_source: &str) -> String {
    let Some(delta) = delta else {
        return "Volume Delta/CVD: unavailable.".to_string();
    };
    format!(
        "Volume Delta/CVD: delta={:.2} delta_ratio={:.2} cvd={:.2} cvd_change={:.2} \
        (buy_volume={:.2}, sell_volume={:.2})\n\
        Volume source: {} — PAXG proxy caveat: this is not true XAU/USD volume; \
        treat it as relative flow only, never as absolute institutional volume.",
        delta.delta,
        delta.delta_ratio,
        delta.cvd,
        delta.cvd_change,
        delta.buy_volume,
        delta.sell_volume,
        if volume_source.trim().is_empty() {
            "UNKNOWN"
        } else {
            volume_source
        }
    )
}

fn format_candidate_block(candidate: Option<&Signal>) -> String {
    let Some(candidate) = candidate else {
        return "No deterministic candidate for this candle after relaxing the soft filters. \
                You may still choose STANDBY or reference a scenario, but do not invent entry/SL/TP."
            .to_string();
    };

    format!(
        "Candidate technical setup (deterministic, relaxed filters):\n\
        side={} entry={:.2} SL={:.2} TP1={:.2} TP2={:.2} RR=1:{:.2} lot={:.2}\n\
        confluence={}/100 volume_surge={:.2}x entry_type={} candle_close={:.2} current_price={}\n\
        price_action={}\n\
        volume_analysis={}",
        candidate.signal_type.as_str(),
        candidate.entry_price,
        candidate.stop_loss,
        candidate.take_profit_1,
        candidate.take_profit_2,
        candidate.risk_reward_ratio,
        candidate.recommended_lot_size,
        candidate.confluence_score,
        candidate.volume_surge_ratio,
        candidate.entry_type.as_str(),
        candidate.candle_close_price,
        format_optional_price(candidate.live_market_price),
        truncate_chars(&candidate.price_action_notes, 240),
        truncate_chars(&candidate.volume_analysis_notes, 180)
    )
}

fn format_context_block(context: Option<&MarketContext>) -> String {
    match context {
        Some(context) => format!(
            "{}\n{}\n{}\nprice_source={} spot_basis={}",
            format_timeframe_context("D1", &context.d1),
            format_timeframe_context("H4", &context.h4),
            format_timeframe_context("H1", &context.h1),
            context.price_source,
            context
                .spot_basis
                .map(|basis| format!("{basis:+.2}"))
                .unwrap_or_else(|| "n/a".to_string())
        ),
        None => "D1/H4/H1 context unavailable (HTF feed not ready).".to_string(),
    }
}

fn format_upcoming_events(events: &[EconomicEvent], now: DateTime<Utc>) -> String {
    let upcoming: Vec<String> = events
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

    if upcoming.is_empty() {
        "none".to_string()
    } else {
        upcoming.join("; ")
    }
}

fn current_price(
    candidate: Option<&Signal>,
    context: Option<&MarketContext>,
    scenarios: Option<&ScenarioResponse>,
) -> Option<f64> {
    candidate
        .and_then(|candidate| candidate.live_market_price)
        .or_else(|| candidate.map(|candidate| candidate.entry_price))
        .or_else(|| scenarios.and_then(|scenarios| scenarios.price))
        .or_else(|| context.map(|context| context.h1.close))
        .filter(|price| price.is_finite() && *price > 0.0)
}

fn context_price_source(context: Option<&MarketContext>, candidate: Option<&Signal>) -> String {
    context
        .map(|context| context.price_source.clone())
        .or_else(|| candidate.map(|candidate| candidate.data_source.clone()))
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

/// Build the comprehensive Chief Market Analyst prompt (pure, no network).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_decision_prompt(
    candidate: Option<&Signal>,
    context: Option<&MarketContext>,
    macro_flow: Option<&MacroFlowSnapshot>,
    scenarios: Option<&ScenarioResponse>,
    rsi: Option<&RsiSnapshot>,
    delta: Option<&DeltaSnapshot>,
    events: &[EconomicEvent],
    now: DateTime<Utc>,
) -> String {
    let vn_now = now + Duration::hours(7);
    let session_label = if (13..=23).contains(&vn_now.hour()) {
        "IN golden session 13:00-23:00 VN"
    } else {
        "OUTSIDE golden session 13:00-23:00 VN"
    };
    let price = format_optional_price(current_price(candidate, context, scenarios));
    let price_source = context_price_source(context, candidate);
    let volume_source = scenarios
        .map(|scenarios| scenarios.volume_source.as_str())
        .filter(|source| !source.trim().is_empty())
        .unwrap_or("UNKNOWN");
    let context_block = format_context_block(context);
    let macro_block = format_macro_flow_block(candidate, macro_flow);
    let rsi_block = format_rsi_block(rsi, context);
    let delta_block = format_delta_block(delta, volume_source);
    let scenario_block = format_scenario_block(scenarios);
    let candidate_block = format_candidate_block(candidate);
    let events_text = format_upcoming_events(events, now);

    format!(
        "You are the Chief Market Analyst (AI) for XAU/USD. You are the PRIMARY decision layer: \
        decide BUY, SELL or STANDBY after reviewing the full collected brief below. \
        Deterministic code retains every hard safety rule.\n\n\
        Current price: {price} | price_source={price_source} | volume_source={volume_source}\n\
        Volume caveat: Binance PAXG volume is a proxy, not true XAU/USD volume; use it as relative flow only.\n\n\
        D1/H4/H1 context (read-only):\n{context_block}\n\n\
        {macro_block}\n\n\
        RSI context (Wilder, closed candles):\n{rsi_block}\n\n\
        {delta_block}\n\n\
        Scenario view (PRIMARY/ALTERNATE/WATCHLIST):\n{scenario_block}\n\n\
        {candidate_block}\n\n\
        Timing: {} UTC | {} (VN) | {}\n\
        Upcoming High-Impact USD events: {}\n\n\
        Hard rules enforced by code (do not attempt to override):\n\
        - SL/TP ordering, minimum R:R, risk budget, daily quota, session window, news kill switch and dedup are validated in code.\n\
        - The AI never sets live SL/TP and must not invent prices. Choose a provided candidate/scenario or STANDBY.\n\
        - If the candidate and scenarios disagree, prefer STANDBY unless one setup clearly has better structure and R:R.\n\
        - If data is missing, contradictory, or the setup is counter-macro without exceptional confluence, prefer STANDBY.\n\
        - If no valid candidate/scenario exists, STANDBY is the only safe action.\n\
        - BUY/SELL means you approve using the provided candidate or selected scenario levels; execution still requires all hard checks to pass.\n\n\
        Return ONLY one valid JSON object with exactly these keys:\n\
        {{\"action\":\"BUY\"|\"SELL\"|\"STANDBY\",\"confidence_score\":0.0,\
        \"selected_scenario\":\"PRIMARY\"|\"ALTERNATE\"|\"WATCHLIST\"|\"NONE\",\
        \"reason\":\"non-empty, <=300 chars\",\
        \"execution_rule\":\"short text or null\",\
        \"macro_flow_alignment\":\"ALIGNED\"|\"COUNTER\"|\"MIXED\"|\"UNKNOWN\",\
        \"macro_flow_note\":\"short text or null\"}}\n\
        action must be uppercase BUY, SELL or STANDBY. confidence_score must be 0.0-1.0. \
        selected_scenario must name the scenario you rely on, or NONE for STANDBY. \
        Keep reason concise and factual; never include markdown or extra keys.",
        now.format("%Y-%m-%d %H:%M"),
        vn_now.format("%Y-%m-%d %H:%M"),
        session_label,
        events_text
    )
}

/// Parse + validate the AI decision JSON. Malformed => None.
pub(crate) fn parse_decision_content(
    content: &str,
    provider: &str,
    model: &str,
) -> Option<AiDecision> {
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

    let selected_scenario = optional_string(&value, "selected_scenario").and_then(|scenario| {
        let normalized = scenario.trim().to_uppercase();
        matches!(
            normalized.as_str(),
            "PRIMARY" | "ALTERNATE" | "WATCHLIST" | "NONE"
        )
        .then_some(normalized)
    });
    let macro_flow_alignment = optional_string(&value, "macro_flow_alignment")
        .and_then(|alignment| normalize_macro_flow_alignment(&alignment));

    Some(AiDecision {
        action,
        confidence_score: confidence.clamp(0.0, 1.0),
        selected_scenario,
        reason: truncate_inclusive(reason, 300),
        execution_rule: optional_string(&value, "execution_rule")
            .map(|rule| truncate_chars(&rule, 200)),
        macro_flow_alignment,
        macro_flow_note: optional_string(&value, "macro_flow_note")
            .map(|note| truncate_chars(&note, 240)),
        provider: provider.to_string(),
        model: model.to_string(),
    })
}

/// Decide whether the AI decision allows the candidate to proceed to the hard
/// risk checks. Mirrors `review_blocks_signal` but for the DECISION verdict.
pub fn decision_blocks_or_approves(
    candidate: &Signal,
    decision: &AiDecision,
    min_confidence: f64,
) -> (bool, String) {
    let action = decision.action.trim().to_uppercase();
    let threshold = review_min_confidence(min_confidence);

    if action == "STANDBY" {
        return (
            true,
            format!(
                "AI Decision chọn STANDBY (confidence {:.0}%): {}",
                decision.confidence_score * 100.0,
                decision.reason
            ),
        );
    }

    if action != candidate.signal_type.as_str() {
        return (
            true,
            format!(
                "AI Decision {} không khớp candidate {} (confidence {:.0}%): {}",
                action,
                candidate.signal_type.as_str(),
                decision.confidence_score * 100.0,
                decision.reason
            ),
        );
    }

    if !decision.confidence_score.is_finite() {
        return (
            true,
            "AI Decision confidence không hợp lệ (non-finite) — chặn để an toàn.".to_string(),
        );
    }

    if decision.confidence_score < threshold {
        return (
            true,
            format!(
                "AI Decision confidence {:.0}% thấp hơn ngưỡng {:.0}%: {}",
                decision.confidence_score * 100.0,
                threshold * 100.0,
                decision.reason
            ),
        );
    }

    (
        false,
        format!(
            "AI Decision đồng ý {} (confidence {:.0}%): {}",
            action,
            decision.confidence_score * 100.0,
            decision.reason
        ),
    )
}

/// Call OpenCode Go for a full BUY/SELL/STANDBY decision.
///
/// Returns None when DECISION mode is disabled, `OPENCODE_GO_API_KEY` is missing,
/// the provider errors/times out, or the response is malformed. In DECISION mode
/// callers must treat None as fail-closed: no trade signal.
#[allow(clippy::too_many_arguments)]
pub async fn review_market_decision(
    candidate: Option<&Signal>,
    context: Option<&MarketContext>,
    macro_flow: Option<&MacroFlowSnapshot>,
    scenarios: Option<&ScenarioResponse>,
    rsi: Option<&RsiSnapshot>,
    delta: Option<&DeltaSnapshot>,
    events: &[EconomicEvent],
    config: &BotConfig,
) -> Option<AiDecision> {
    if !ai_decision_enabled(config) {
        return None;
    }

    let settings = resolve_review_settings(
        &std::env::var("OPENCODE_GO_API_KEY").unwrap_or_default(),
        &std::env::var("OPENCODE_GO_BASE_URL").unwrap_or_default(),
        &std::env::var("AI_SIGNAL_REVIEW_MODEL").unwrap_or_default(),
    )?;

    let now = Utc::now();
    let prompt = build_decision_prompt(
        candidate, context, macro_flow, scenarios, rsi, delta, events, now,
    );
    let content = post_ai_chat_completion(
        &settings,
        "You are the Chief Market Analyst for an XAU/USD signal bot. Decide BUY, SELL or STANDBY from the supplied brief. Never invent prices or change live SL/TP. Return pure valid JSON only, with no markdown wrapping.",
        &prompt,
        &review_session_id(now),
        0.2,
        "AI Decision",
    )
    .await?;

    let decision = parse_decision_content(&content, "opencode_go", &settings.model);
    match decision.as_ref() {
        Some(decision) => info!(
            "🧠 [AI Decision] action={} confidence={:.2} scenario={} model={} macro_alignment={}",
            decision.action,
            decision.confidence_score,
            decision.selected_scenario.as_deref().unwrap_or("NONE"),
            decision.model,
            decision.macro_flow_alignment.as_deref().unwrap_or("n/a")
        ),
        None => warn!("⚠️ [AI Decision] Phản hồi AI không hợp lệ — fail-closed (không tín hiệu)."),
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macro_flow::{CotSnapshot, MacroFlowScorecard, MacroMetric};
    use crate::market_context::TimeframeContext;
    use crate::models::{EntryType, SignalStatus, SignalType, Timeframe};
    use crate::rsi::divergence_label;
    use crate::scenarios::{Scenario, ScenarioMarket};
    use chrono::TimeZone;

    fn sample_signal() -> Signal {
        Signal {
            id: "SIG-AI-DECISION".to_string(),
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
            delta_ratio: None,
            cvd: None,
            delta: None,
            rsi_value: None,
            rsi_state: None,
            rsi_divergence: None,
            macro_flow_alignment: None,
            macro_flow_note: None,
        }
    }

    fn decision(action: &str, confidence: f64) -> AiDecision {
        AiDecision {
            action: action.to_string(),
            confidence_score: confidence,
            selected_scenario: Some("PRIMARY".to_string()),
            reason: "structure agrees".to_string(),
            execution_rule: None,
            macro_flow_alignment: None,
            macro_flow_note: None,
            provider: "opencode_go".to_string(),
            model: "gemini-4-argon".to_string(),
        }
    }

    fn macro_metric(key: &str, value: f64, change_5d: f64, change_5d_pct: f64) -> MacroMetric {
        MacroMetric {
            key: key.to_string(),
            label: key.to_string(),
            value: Some(value),
            unit: "%".to_string(),
            change_1d: None,
            change_5d: Some(change_5d),
            change_5d_pct: Some(change_5d_pct),
            volume_ratio_20d: None,
            date: Some("2026-11-11".to_string()),
            source: "TEST".to_string(),
            note: String::new(),
        }
    }

    fn macro_snapshot() -> MacroFlowSnapshot {
        MacroFlowSnapshot {
            updated_at: "2026-11-12T00:00:00Z".to_string(),
            fred_ok: true,
            yahoo_ok: true,
            cot_ok: true,
            metrics: vec![
                macro_metric("real_yield_10y", 2.66, 0.04, 1.53),
                macro_metric("dxy", 103.0, 1.45, 1.43),
                macro_metric("gold", 3980.0, -133.8, -3.25),
                macro_metric("gld", 380.0, -10.0, -2.5),
                macro_metric("iau", 38.0, -1.0, -2.4),
                macro_metric("vix", 18.0, 0.0, 0.0),
                macro_metric("gvz", 21.0, 0.0, 0.0),
                macro_metric("mmf", 7_100_000.0, 0.0, 0.0),
            ],
            cot: Some(CotSnapshot {
                report_date: "2026-11-03".to_string(),
                open_interest: 399_169.0,
                noncommercial_long: 220_530.0,
                noncommercial_short: 40_982.0,
                noncommercial_net: 179_548.0,
                noncommercial_net_change: -6_903.0,
                net_pct_oi: 44.9806,
                open_interest_change: 2_359.0,
            }),
            scorecard: MacroFlowScorecard {
                macro_bias: "BEARISH".to_string(),
                confidence: 95,
                score: -4,
                direction: "DOWN".to_string(),
                driver: "real yield ↑, DXY ↑".to_string(),
                flow_quality: "CONFIRMING".to_string(),
                risk_regime: "NEUTRAL".to_string(),
                rate_regime: "TIGHTENING".to_string(),
                usd_regime: "STRONG".to_string(),
                vol_regime: "NORMAL".to_string(),
                notes: Vec::new(),
            },
            warnings: Vec::new(),
            sources: Vec::new(),
        }
    }

    fn scenario(role: &str, valid: bool) -> Scenario {
        Scenario {
            id: format!("SCN-{role}"),
            name: format!("{role} breakout"),
            bias: "BULLISH".to_string(),
            trigger: "Đóng M15 trên 2701.00".to_string(),
            condition: "Phá vỡ kháng cự gần nhất".to_string(),
            entry: Some(2701.0),
            stop_loss: Some(2691.0),
            take_profit_1: Some(2721.0),
            take_profit_2: Some(2731.0),
            risk_reward: Some(2.0),
            trigger_price: Some(2701.0),
            trigger_direction: "ABOVE".to_string(),
            trigger_timeframe: "M15".to_string(),
            invalidation: "Đóng M15 dưới 2691.00".to_string(),
            confidence: 82,
            macro_alignment: "ALIGNED".to_string(),
            valid,
            role: role.to_string(),
            priority: 90,
            notes: vec!["Structure agrees".to_string()],
        }
    }

    fn scenario_response() -> ScenarioResponse {
        ScenarioResponse {
            status: "ok".to_string(),
            generated_at: "2026-11-12T00:00:00Z".to_string(),
            price: Some(2684.5),
            source: "BINANCE_PAXG".to_string(),
            volume_source: "BINANCE_PAXG_PROXY".to_string(),
            market: Some(ScenarioMarket {
                h4_bias: "BULLISH".to_string(),
                h1_bias: "BULLISH".to_string(),
                d1_bias: "BULLISH".to_string(),
                h4_atr: 18.0,
                nearest_support: Some(2670.0),
                nearest_resistance: Some(2700.0),
                rsi_m15: Some(55.0),
                rsi_state: "Neutral".to_string(),
                macro_bias: "BEARISH".to_string(),
                macro_confidence: 95,
            }),
            scenarios: vec![
                scenario("PRIMARY", true),
                scenario("ALTERNATE", true),
                scenario("WATCHLIST", false),
            ],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn decision_mode_requires_enabled_and_decision_value() {
        let mut config = BotConfig {
            ai_signal_review_enabled: true,
            ..BotConfig::default()
        };
        assert!(ai_decision_enabled(&config));

        config.ai_review_mode = "VETO_ONLY".to_string();
        assert!(!ai_decision_enabled(&config));

        config.ai_review_mode = "DECISION".to_string();
        config.ai_signal_review_enabled = false;
        assert!(!ai_decision_enabled(&config));
    }

    #[test]
    fn relaxed_config_only_disables_soft_candidate_filters() {
        let config = BotConfig {
            volume_filter_enabled: true,
            macro_bias: "BULLISH".to_string(),
            min_risk_reward: 2.5,
            risk_percentage: 1.5,
            max_signals_per_day: 2,
            ..BotConfig::default()
        };
        let relaxed = relaxed_candidate_config(&config);

        assert!(!relaxed.volume_filter_enabled);
        assert_eq!(relaxed.macro_bias, "NEUTRAL");
        assert_eq!(relaxed.min_risk_reward, config.min_risk_reward);
        assert_eq!(relaxed.risk_percentage, config.risk_percentage);
        assert_eq!(relaxed.max_signals_per_day, config.max_signals_per_day);
        assert!(config.volume_filter_enabled);
        assert_eq!(config.macro_bias, "BULLISH");
    }

    #[test]
    fn prompt_contains_full_brief_and_output_schema() {
        let now = Utc
            .with_ymd_and_hms(2026, 11, 12, 14, 30, 0)
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
        let context = crate::market_context::build_market_context(&[], &[], &[], "TEST", None);
        let rsi = RsiSnapshot {
            value: Some(76.4),
            state: "Overbought".to_string(),
            divergence: crate::rsi::RsiDivergence::Bearish,
        };
        let delta = DeltaSnapshot {
            buy_volume: 1200.0,
            sell_volume: 800.0,
            delta: 400.0,
            delta_ratio: 0.2,
            cvd: 1500.0,
            cvd_change: 300.0,
        };
        let prompt = build_decision_prompt(
            Some(&sample_signal()),
            Some(&context),
            Some(&macro_snapshot()),
            Some(&scenario_response()),
            Some(&rsi),
            Some(&delta),
            &events,
            now,
        );

        assert!(prompt.contains("Chief Market Analyst"));
        assert!(prompt.contains("Current price: 2684.80"));
        assert!(prompt.contains("volume_source=BINANCE_PAXG_PROXY"));
        assert!(prompt.contains("D1: bias="));
        assert!(prompt.contains("Macro & Flow scorecard:"));
        assert!(prompt.contains("real_yield_10y=2.66"));
        assert!(prompt.contains("GLD="));
        assert!(prompt.contains("MMF="));
        assert!(prompt.contains("rsi_m15: value=76.4"));
        assert!(prompt.contains("70/30 means >=70 Overbought"));
        assert!(prompt.contains("delta=400.00"));
        assert!(prompt.contains("PAXG proxy caveat"));
        assert!(prompt.contains("[PRIMARY] PRIMARY breakout"));
        assert!(prompt.contains("[ALTERNATE]"));
        assert!(prompt.contains("[WATCHLIST]"));
        assert!(prompt.contains("Candidate technical setup"));
        assert!(prompt.contains("side=BUY"));
        assert!(prompt.contains("US Core CPI m/m"));
        assert!(prompt.contains("IN golden session 13:00-23:00 VN"));
        assert!(prompt.contains("Hard rules enforced by code"));
        assert!(prompt.contains("\"selected_scenario\""));
        assert!(prompt.contains("\"macro_flow_alignment\""));
    }

    #[test]
    fn prompt_handles_missing_candidate_and_data() {
        let now = Utc
            .with_ymd_and_hms(2026, 11, 12, 2, 0, 0)
            .single()
            .expect("valid timestamp");
        let prompt = build_decision_prompt(None, None, None, None, None, None, &[], now);

        assert!(prompt.contains("Current price: n/a"));
        assert!(prompt.contains("D1/H4/H1 context unavailable"));
        assert!(prompt.contains("Macro & Flow: unavailable"));
        assert!(prompt.contains("rsi_m15: n/a"));
        assert!(prompt.contains("Volume Delta/CVD: unavailable"));
        assert!(prompt.contains("Scenario view unavailable"));
        assert!(prompt.contains("No deterministic candidate"));
        assert!(prompt.contains("OUTSIDE golden session 13:00-23:00 VN"));
    }

    #[test]
    fn parses_valid_decision_and_clamps_confidence() {
        let content = r#"{
            "action": "buy",
            "confidence_score": 1.4,
            "selected_scenario": " primary ",
            "reason": "  D1/H4 cùng hướng tăng.  ",
            "execution_rule": "  Wait for M15 close above 2701  ",
            "macro_flow_alignment": " aligned ",
            "macro_flow_note": "  Macro ủng hộ.  "
        }"#;
        let parsed = parse_decision_content(content, "opencode_go", "test-model")
            .expect("valid decision JSON");

        assert_eq!(parsed.action, "BUY");
        assert_eq!(parsed.confidence_score, 1.0);
        assert_eq!(parsed.selected_scenario.as_deref(), Some("PRIMARY"));
        assert_eq!(parsed.reason, "D1/H4 cùng hướng tăng.");
        assert_eq!(
            parsed.execution_rule.as_deref(),
            Some("Wait for M15 close above 2701")
        );
        assert_eq!(parsed.macro_flow_alignment.as_deref(), Some("ALIGNED"));
        assert_eq!(parsed.macro_flow_note.as_deref(), Some("Macro ủng hộ."));
        assert_eq!(parsed.provider, "opencode_go");
        assert_eq!(parsed.model, "test-model");
    }

    #[test]
    fn normalizes_selected_scenario_and_alignment() {
        for (scenario, expected) in [
            ("ALTERNATE", Some("ALTERNATE")),
            ("watchlist", Some("WATCHLIST")),
            ("NONE", Some("NONE")),
            ("bogus", None),
        ] {
            let content = format!(
                r#"{{"action":"STANDBY","confidence_score":0.5,"selected_scenario":"{scenario}","macro_flow_alignment":"sideways","reason":"wait"}}"#
            );
            let parsed = parse_decision_content(&content, "opencode_go", "test-model")
                .expect("valid decision");
            assert_eq!(parsed.selected_scenario.as_deref(), expected);
            assert_eq!(parsed.macro_flow_alignment, None);
        }
    }

    #[test]
    fn rejects_malformed_decisions() {
        for content in [
            "not json",
            r#"{"action":"HOLD","confidence_score":0.9,"reason":"x"}"#,
            r#"{"action":"BUY","confidence_score":0.9,"reason":"  "}"#,
            r#"{"action":"BUY","reason":"missing confidence"}"#,
            r#"{"confidence_score":0.9,"reason":"missing action"}"#,
            r#"{"action":"BUY","confidence_score":"high","reason":"bad type"}"#,
        ] {
            assert!(
                parse_decision_content(content, "opencode_go", "test-model").is_none(),
                "expected None for {content}"
            );
        }
    }

    #[test]
    fn decision_gate_blocks_standby_opposite_and_low_confidence() {
        let candidate = sample_signal();

        let (blocked, reason) =
            decision_blocks_or_approves(&candidate, &decision("STANDBY", 0.99), 0.75);
        assert!(blocked);
        assert!(reason.contains("STANDBY"));

        let (blocked, reason) =
            decision_blocks_or_approves(&candidate, &decision("SELL", 0.95), 0.75);
        assert!(blocked);
        assert!(reason.contains("không khớp"));

        let (blocked, reason) =
            decision_blocks_or_approves(&candidate, &decision("BUY", 0.60), 0.75);
        assert!(blocked);
        assert!(reason.contains("thấp hơn ngưỡng"));

        let (blocked, reason) =
            decision_blocks_or_approves(&candidate, &decision("BUY", 0.80), 0.75);
        assert!(!blocked);
        assert!(reason.contains("đồng ý"));
    }

    #[test]
    fn decision_gate_is_case_insensitive_and_clamps_threshold() {
        let candidate = sample_signal();
        let (blocked, _) = decision_blocks_or_approves(&candidate, &decision("buy", 0.90), 0.75);
        assert!(!blocked);

        let (blocked, _) = decision_blocks_or_approves(&candidate, &decision("BUY", 0.55), 0.1);
        assert!(!blocked, "threshold must clamp to 0.5, allowing 0.55");
    }

    #[test]
    fn decision_gate_rejects_non_finite_confidence() {
        let candidate = sample_signal();
        let (blocked, reason) =
            decision_blocks_or_approves(&candidate, &decision("BUY", f64::NAN), 0.75);
        assert!(blocked);
        assert!(reason.contains("không hợp lệ"));
    }

    #[tokio::test]
    async fn review_market_decision_returns_none_when_disabled_without_network() {
        let config = BotConfig {
            ai_signal_review_enabled: false,
            ..BotConfig::default()
        };
        let result = review_market_decision(None, None, None, None, None, None, &[], &config).await;
        assert!(result.is_none());
    }

    #[test]
    fn timeframe_context_formatter_is_shared_with_veto_review() {
        let context = TimeframeContext {
            timeframe: "H4".to_string(),
            bias: "BULLISH".to_string(),
            close: 2700.0,
            ema50: 2680.0,
            ema200: 2650.0,
            channel_position: 0.8,
            atr14: 18.0,
            nearest_support: Some(2670.0),
            nearest_resistance: Some(2710.0),
            candles: 100,
            last_candle_timestamp: 1,
        };
        let formatted = format_timeframe_context("H4", &context);
        assert!(formatted.starts_with("H4: bias=BULLISH"));
    }

    #[test]
    fn rsi_label_helper_reports_divergence() {
        let snapshot = RsiSnapshot {
            value: Some(28.0),
            state: "Oversold".to_string(),
            divergence: crate::rsi::RsiDivergence::Bullish,
        };
        assert_eq!(
            format_rsi_value(&snapshot),
            "value=28.0 state=Oversold divergence=Bullish"
        );
        assert_eq!(divergence_label(snapshot.divergence), "Bullish");
    }
}
