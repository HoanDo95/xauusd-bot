//! On-demand AI entry check.
//!
//! This is a manual, read-only "what would the AI do right now?" view. It builds
//! the same brief as Scanner Start, asks the AI for a BUY/SELL/STANDBY decision,
//! and derives a reference entry plan from the deterministic candidate or a
//! scenario. It never inserts/persists a signal, never sends Telegram, never
//! consumes the Sniper quota and never places a trade. The AI never invents or
//! changes live SL/TP: plan levels always come from the candidate/scenario.

use super::{current_live_view, current_scenarios, AppState};
use crate::ai_decision::{
    ai_decision_enabled, relaxed_candidate_config, review_market_decision, AiDecision,
};
use crate::analyzer::GoldMarketAnalyzer;
use crate::models::{BotConfig, Signal, SignalType};
use crate::risk_manager::PositionSizer;
use crate::rsi::rsi_snapshot;
use crate::scenarios::{Scenario, ScenarioResponse};
use crate::signal_validation::{validate_risk_budget, validate_signal_levels};
use crate::volume_delta::compute_delta;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::warn;

pub(crate) const AI_DECISION_DISABLED_MESSAGE: &str = "AI Decision Mode đang tắt.";
pub(crate) const AI_DECISION_UNAVAILABLE_MESSAGE: &str =
    "AI không phản hồi hoặc phản hồi không hợp lệ.";
pub(crate) const STANDBY_PLAN_NOTE: &str = "AI STANDBY — kế hoạch tham khảo, chưa phải entry";

/// Reference entry plan derived from a deterministic candidate or scenario.
/// It is display-only; it is never inserted as a trade signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryPlan {
    /// BUY | SELL
    pub side: String,
    /// CANDIDATE | PRIMARY | ALTERNATE | WATCHLIST
    pub source: String,
    pub entry: f64,
    pub stop_loss: f64,
    pub take_profit_1: f64,
    pub take_profit_2: f64,
    pub risk_reward: f64,
    pub valid: bool,
    pub note: String,
}

/// Full on-demand AI entry check result. Read-only and never a trade signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryCheckResult {
    pub decision: AiDecision,
    pub live_view: Value,
    pub scenarios: ScenarioResponse,
    pub entry_plan: Option<EntryPlan>,
    pub generated_at: String,
}

#[derive(Debug, Clone, Copy)]
struct PlanLevels {
    entry: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
    risk_reward: Option<f64>,
}

fn action_signal_type(action: &str) -> Option<SignalType> {
    match action.trim().to_uppercase().as_str() {
        "BUY" => Some(SignalType::Buy),
        "SELL" => Some(SignalType::Sell),
        _ => None,
    }
}

fn scenario_signal_type(bias: &str) -> Option<SignalType> {
    match bias.trim().to_uppercase().as_str() {
        "BULLISH" => Some(SignalType::Buy),
        "BEARISH" => Some(SignalType::Sell),
        _ => None,
    }
}

fn bias_matches_action(bias: &str, action: &str) -> bool {
    match action_signal_type(action) {
        Some(side) => scenario_signal_type(bias) == Some(side),
        None => false,
    }
}

fn scenario_levels(scenario: &Scenario) -> Option<PlanLevels> {
    let entry = scenario
        .entry
        .filter(|value| value.is_finite() && *value > 0.0)?;
    let stop_loss = scenario
        .stop_loss
        .filter(|value| value.is_finite() && *value > 0.0)?;
    let take_profit_1 = scenario
        .take_profit_1
        .filter(|value| value.is_finite() && *value > 0.0)?;
    let take_profit_2 = scenario
        .take_profit_2
        .filter(|value| value.is_finite() && *value > 0.0)?;

    Some(PlanLevels {
        entry,
        stop_loss,
        take_profit_1,
        take_profit_2,
        risk_reward: scenario.risk_reward,
    })
}

/// Build one reference plan and apply the same structural/risk checks the live
/// scanner uses. A failed check keeps the plan visible with `valid=false` and a
/// human-readable reason, so the dashboard/Telegram can explain why it is unsafe.
fn build_entry_plan(
    side: SignalType,
    source: &str,
    levels: PlanLevels,
    note: String,
    config: &BotConfig,
) -> EntryPlan {
    let risk = (levels.entry - levels.stop_loss).abs();
    let computed_risk_reward = if risk > 0.0 && risk.is_finite() {
        (levels.take_profit_1 - levels.entry).abs() / risk
    } else {
        0.0
    };
    let risk_reward = levels
        .risk_reward
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(computed_risk_reward);

    let mut plan = EntryPlan {
        side: side.as_str().to_string(),
        source: source.trim().to_uppercase(),
        entry: levels.entry,
        stop_loss: levels.stop_loss,
        take_profit_1: levels.take_profit_1,
        take_profit_2: levels.take_profit_2,
        risk_reward,
        valid: false,
        note,
    };

    let mut errors = Vec::new();
    if let Err(err) = validate_signal_levels(
        side,
        plan.entry,
        plan.stop_loss,
        plan.take_profit_1,
        plan.take_profit_2,
        config.min_risk_reward,
    ) {
        errors.push(err);
    }

    if risk.is_finite() && risk > 0.0 {
        let sizing = PositionSizer::calculate_lot_size(
            config.account_balance,
            config.risk_percentage,
            risk,
            (plan.take_profit_1 - plan.entry).abs(),
            (plan.take_profit_2 - plan.entry).abs(),
            plan.entry,
            config.account_leverage,
        );
        if let Err(err) = validate_risk_budget(config, &sizing) {
            errors.push(err);
        }
    } else {
        errors.push("risk distance không hợp lệ".to_string());
    }

    plan.valid = errors.is_empty();
    if !plan.valid {
        let reason = errors.join("; ");
        plan.note = if plan.note.trim().is_empty() {
            format!("Không hợp lệ: {reason}")
        } else {
            format!("{} — không hợp lệ: {reason}", plan.note)
        };
    }

    plan
}

fn plan_from_candidate(candidate: &Signal, config: &BotConfig) -> EntryPlan {
    let side = candidate.signal_type;
    build_entry_plan(
        side,
        "CANDIDATE",
        PlanLevels {
            entry: candidate.entry_price,
            stop_loss: candidate.stop_loss,
            take_profit_1: candidate.take_profit_1,
            take_profit_2: candidate.take_profit_2,
            risk_reward: Some(candidate.risk_reward_ratio),
        },
        format!(
            "Kế hoạch tham khảo từ candidate {} (relaxed filters).",
            side.as_str()
        ),
        config,
    )
}

fn plan_from_scenario(scenario: &Scenario, config: &BotConfig, note: String) -> Option<EntryPlan> {
    let side = scenario_signal_type(&scenario.bias)?;
    let levels = scenario_levels(scenario)?;
    let source = if scenario.role.trim().is_empty() {
        "PRIMARY".to_string()
    } else {
        scenario.role.trim().to_uppercase()
    };
    Some(build_entry_plan(side, &source, levels, note, config))
}

/// Pure plan selection shared by `run_entry_check` and the unit tests.
///
/// - AI BUY/SELL + matching candidate side => CANDIDATE plan.
/// - otherwise the AI-selected PRIMARY/ALTERNATE/WATCHLIST scenario, only when
///   its bias matches the AI direction.
/// - AI STANDBY => PRIMARY scenario as a reference plan, never an entry.
pub(crate) fn select_entry_plan(
    decision: &AiDecision,
    candidate: Option<&Signal>,
    scenarios: &ScenarioResponse,
    config: &BotConfig,
) -> Option<EntryPlan> {
    let action = decision.action.trim().to_uppercase();

    match action.as_str() {
        "BUY" | "SELL" => {
            if let Some(candidate) =
                candidate.filter(|candidate| candidate.signal_type.as_str() == action.as_str())
            {
                return Some(plan_from_candidate(candidate, config));
            }

            let role = decision
                .selected_scenario
                .as_deref()
                .map(str::trim)
                .map(str::to_uppercase)?;
            if !matches!(role.as_str(), "PRIMARY" | "ALTERNATE" | "WATCHLIST") {
                return None;
            }

            let scenario = scenarios.scenarios.iter().find(|scenario| {
                scenario.role.eq_ignore_ascii_case(&role)
                    && bias_matches_action(&scenario.bias, &action)
                    && scenario_levels(scenario).is_some()
            })?;
            let note = format!("Kế hoạch tham khảo từ kịch bản {role}.");
            plan_from_scenario(scenario, config, note)
        }
        "STANDBY" => {
            let scenario = scenarios
                .scenarios
                .iter()
                .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"))?;
            scenario_levels(scenario)?;
            plan_from_scenario(scenario, config, STANDBY_PLAN_NOTE.to_string())
        }
        _ => None,
    }
}

/// Build the full AI brief, ask for one decision, and derive a reference plan.
///
/// Read-only by design: the only state written is `last_ai_decision` (RAM), so
/// `/api/action-view` can display the latest verdict. No signal is inserted, no
/// Telegram message is sent and the daily quota is untouched.
pub(crate) async fn run_entry_check(state: &AppState) -> Result<EntryCheckResult, String> {
    let config = state.config.read().await.clone();
    if !ai_decision_enabled(&config) {
        return Err(AI_DECISION_DISABLED_MESSAGE.to_string());
    }

    let candles = state.candles.read().await.clone();
    let live_price = *state.live_price.read().await;
    let mut context = state.market_context.read().await.clone();
    let macro_flow = state.macro_flow.read().await.clone();
    let events = state.news_filter.read().await.calendar_events.clone();
    let now = Utc::now();
    let now_ms = now.timestamp_millis();

    let rsi = rsi_snapshot(
        &candles,
        config.rsi_period,
        config.rsi_overbought,
        config.rsi_oversold,
        config.rsi_divergence_lookback,
        config.rsi_pivot_k,
    );
    if let Some(context) = context.as_mut() {
        context.rsi_m15 = Some(rsi.clone());
    }
    let delta = compute_delta(&candles, config.volume_delta_lookback);
    let relaxed = relaxed_candidate_config(&config);
    let candidate =
        GoldMarketAnalyzer::analyze_market_with_live_price(&candles, &relaxed, live_price, now_ms);

    let live_view = current_live_view(state).await;
    let scenarios = current_scenarios(state).await;

    let decision = review_market_decision(
        candidate.as_ref(),
        context.as_ref(),
        macro_flow.as_ref(),
        Some(&scenarios),
        Some(&rsi),
        Some(&delta),
        &events,
        &config,
    )
    .await
    .ok_or_else(|| {
        warn!("⚠️ [AI Entry Check] Không có quyết định AI — không có kế hoạch tham khảo.");
        AI_DECISION_UNAVAILABLE_MESSAGE.to_string()
    })?;

    *state.last_ai_decision.write().await = Some(decision.clone());
    let entry_plan = select_entry_plan(&decision, candidate.as_ref(), &scenarios, &config);

    Ok(EntryCheckResult {
        decision,
        live_view,
        scenarios,
        entry_plan,
        generated_at: now.to_rfc3339(),
    })
}

/// POST /api/ai/entry-check — manual on-demand AI decision + reference plan.
///
/// 200 `{"status":"ok","result":{...}}` or 503
/// `{"status":"unavailable","message":"..."}`. No Telegram, signal or quota.
pub(super) async fn entry_check(State(state): State<AppState>) -> impl IntoResponse {
    match run_entry_check(&state).await {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "result": result
            })),
        ),
        Err(message) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "unavailable",
                "message": message
            })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{EntryType, SignalStatus, Timeframe};
    use crate::scenarios::ScenarioMarket;

    fn config() -> BotConfig {
        BotConfig {
            account_balance: 10_000.0,
            risk_percentage: 1.0,
            account_leverage: 100.0,
            min_risk_reward: 2.0,
            ..BotConfig::default()
        }
    }

    fn candidate(
        side: SignalType,
        entry: f64,
        stop_loss: f64,
        take_profit_1: f64,
        take_profit_2: f64,
        risk_reward: f64,
    ) -> Signal {
        let risk = (entry - stop_loss).abs();
        Signal {
            id: "SIG-ENTRY-CHECK".to_string(),
            symbol: "XAU/USD".to_string(),
            signal_type: side,
            timeframe: Timeframe::M15,
            entry_price: entry,
            stop_loss,
            take_profit_1,
            take_profit_2,
            risk_points: risk,
            reward_points_tp1: (take_profit_1 - entry).abs(),
            reward_points_tp2: (take_profit_2 - entry).abs(),
            risk_reward_ratio: risk_reward,
            volume_surge_ratio: 1.8,
            recommended_lot_size: 0.09,
            risk_amount_usd: 94.5,
            potential_profit_tp1_usd: 189.0,
            potential_profit_tp2_usd: 283.5,
            price_action_notes: "Bullish pinbar".to_string(),
            volume_analysis_notes: "Volume 1.8x MA20".to_string(),
            full_rationale: "R:R 1:2".to_string(),
            status: SignalStatus::Active,
            created_at: Utc::now(),
            telegram_message_id: None,
            dedup_key: "XAU/USD_M15_1".to_string(),
            candle_timestamp: 1,
            is_simulated: false,
            confluence_score: 90,
            outcome_at: None,
            entry_type: EntryType::Market,
            candle_close_price: entry,
            live_market_price: Some(entry),
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

    fn decision(action: &str, selected_scenario: Option<&str>) -> AiDecision {
        AiDecision {
            action: action.to_string(),
            confidence_score: 0.85,
            selected_scenario: selected_scenario.map(str::to_string),
            reason: "structure agrees".to_string(),
            execution_rule: None,
            macro_flow_alignment: Some("ALIGNED".to_string()),
            macro_flow_note: None,
            provider: "opencode_go".to_string(),
            model: "test-model".to_string(),
        }
    }

    fn scenario(role: &str, bias: &str) -> Scenario {
        let (entry, stop_loss, take_profit_1, take_profit_2) = match bias {
            "BEARISH" => (2701.0, 2711.0, 2681.0, 2671.0),
            _ => (2701.0, 2691.0, 2721.0, 2731.0),
        };
        Scenario {
            id: format!("SCN-{role}"),
            name: format!("{role} setup"),
            bias: bias.to_string(),
            trigger: "M15 close confirmation".to_string(),
            condition: "structure".to_string(),
            entry: Some(entry),
            stop_loss: Some(stop_loss),
            take_profit_1: Some(take_profit_1),
            take_profit_2: Some(take_profit_2),
            risk_reward: Some(2.0),
            trigger_price: Some(entry),
            trigger_direction: "ABOVE".to_string(),
            trigger_timeframe: "M15".to_string(),
            invalidation: "M15 close back through the level".to_string(),
            confidence: 85,
            macro_alignment: "ALIGNED".to_string(),
            valid: true,
            role: role.to_string(),
            priority: 90,
            notes: Vec::new(),
        }
    }

    fn scenario_response(scenarios: Vec<Scenario>) -> ScenarioResponse {
        ScenarioResponse {
            status: "ok".to_string(),
            generated_at: "2026-11-12T00:00:00Z".to_string(),
            price: Some(2700.0),
            source: "BINANCE_PAXG".to_string(),
            volume_source: "BINANCE_PAXG_PROXY".to_string(),
            market: Some(ScenarioMarket {
                h4_bias: "BULLISH".to_string(),
                h1_bias: "BULLISH".to_string(),
                d1_bias: "BULLISH".to_string(),
                h4_atr: 18.0,
                nearest_support: Some(2690.0),
                nearest_resistance: Some(2710.0),
                rsi_m15: Some(55.0),
                rsi_state: "Neutral".to_string(),
                macro_bias: "BULLISH".to_string(),
                macro_confidence: 80,
            }),
            scenarios,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn ai_buy_with_matching_candidate_builds_candidate_plan() {
        let candidate = candidate(SignalType::Buy, 2684.5, 2674.0, 2705.5, 2716.0, 2.0);
        let decision = decision("BUY", Some("PRIMARY"));
        let scenarios = scenario_response(vec![scenario("PRIMARY", "BULLISH")]);

        let plan = select_entry_plan(&decision, Some(&candidate), &scenarios, &config())
            .expect("candidate plan");

        assert_eq!(plan.side, "BUY");
        assert_eq!(plan.source, "CANDIDATE");
        assert_eq!(plan.entry, 2684.5);
        assert_eq!(plan.stop_loss, 2674.0);
        assert_eq!(plan.take_profit_1, 2705.5);
        assert_eq!(plan.take_profit_2, 2716.0);
        assert_eq!(plan.risk_reward, 2.0);
        assert!(
            plan.valid,
            "valid candidate must pass level + budget checks"
        );
        assert!(plan.note.contains("candidate"));
    }

    #[test]
    fn ai_buy_without_candidate_uses_selected_primary_scenario() {
        let decision = decision("BUY", Some("PRIMARY"));
        let scenarios = scenario_response(vec![scenario("PRIMARY", "BULLISH")]);

        let plan =
            select_entry_plan(&decision, None, &scenarios, &config()).expect("scenario plan");

        assert_eq!(plan.side, "BUY");
        assert_eq!(plan.source, "PRIMARY");
        assert_eq!(plan.entry, 2701.0);
        assert_eq!(plan.stop_loss, 2691.0);
        assert_eq!(plan.take_profit_1, 2721.0);
        assert_eq!(plan.take_profit_2, 2731.0);
        assert!(plan.valid);
        assert!(plan.note.contains("PRIMARY"));
    }

    #[test]
    fn ai_standby_uses_primary_reference_plan() {
        let decision = decision("STANDBY", None);
        let scenarios = scenario_response(vec![scenario("PRIMARY", "BULLISH")]);

        let plan =
            select_entry_plan(&decision, None, &scenarios, &config()).expect("reference plan");

        assert_eq!(plan.side, "BUY");
        assert_eq!(plan.source, "PRIMARY");
        assert!(plan.valid);
        assert!(plan.note.contains("AI STANDBY"));
        assert!(plan.note.contains("chưa phải entry"));
    }

    #[test]
    fn invalid_candidate_levels_mark_plan_invalid_with_note() {
        // Reversed BUY levels: the plan stays visible but must be flagged unsafe.
        let candidate = candidate(SignalType::Buy, 2400.0, 2410.0, 2380.0, 2370.0, 2.0);
        let decision = decision("BUY", None);
        let scenarios = scenario_response(Vec::new());

        let plan = select_entry_plan(&decision, Some(&candidate), &scenarios, &config())
            .expect("invalid plan is still returned for display");

        assert!(!plan.valid);
        assert!(plan.note.contains("không hợp lệ"));
        assert!(plan.note.contains("BUY levels"));
    }

    #[test]
    fn ai_buy_does_not_use_opposite_bias_scenario() {
        let decision = decision("BUY", Some("PRIMARY"));
        let scenarios = scenario_response(vec![scenario("PRIMARY", "BEARISH")]);

        assert!(select_entry_plan(&decision, None, &scenarios, &config()).is_none());
    }

    #[test]
    fn opposite_candidate_falls_back_to_matching_scenario() {
        let candidate = candidate(SignalType::Sell, 2701.0, 2711.0, 2681.0, 2671.0, 2.0);
        let decision = decision("BUY", Some("PRIMARY"));
        let scenarios = scenario_response(vec![scenario("PRIMARY", "BULLISH")]);

        let plan = select_entry_plan(&decision, Some(&candidate), &scenarios, &config())
            .expect("matching scenario plan");

        assert_eq!(plan.source, "PRIMARY");
        assert_eq!(plan.side, "BUY");
    }

    #[test]
    fn no_plan_when_no_candidate_and_no_matching_scenario() {
        let decision = decision("SELL", Some("WATCHLIST"));
        let scenarios = scenario_response(vec![scenario("PRIMARY", "BULLISH")]);

        assert!(select_entry_plan(&decision, None, &scenarios, &config()).is_none());
    }
}
