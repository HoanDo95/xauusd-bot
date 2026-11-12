use super::{current_live_view, current_scenarios, AppState};
use crate::macro_flow::{align_signal_with_macro_flow, MacroFlowSnapshot};
use crate::models::Signal;
use crate::scenarios::{Scenario, ScenarioResponse};
use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;
use serde_json::{json, Value};

const VOLUME_PROXY_WARNING: &str =
    "Volume là proxy PAXG, không phải volume XAUUSD thật; chỉ nên xem là tương đối.";
const NO_DATA_FALLBACK: &str = "Chưa đủ nến để phân tích.";
const NO_TRADE_HEADLINE: &str = "Đứng ngoài";

/// GET /api/action-view — one read-only "Should I enter now?" summary.
///
/// This handler only reports the current live view, scenario view and macro/flow
/// context. It never sends Telegram, inserts/persists a signal, consumes the
/// daily Sniper quota, calls the AI review or places an order.
pub(super) async fn get_action_view(State(state): State<AppState>) -> impl IntoResponse {
    let live_view = current_live_view(&state).await;
    let scenarios = current_scenarios(&state).await;
    let macro_flow = state.macro_flow.read().await.clone();
    let source = state.active_data_source.read().await.clone();
    let volume_source = state.active_volume_source.read().await.clone();
    let trigger_alert_enabled = state.config.read().await.action_trigger_alert_enabled;

    Json(build_action_view(
        &live_view,
        &scenarios,
        macro_flow.as_ref(),
        &source,
        &volume_source,
        trigger_alert_enabled,
    ))
}

/// Map the active volume feed to a display quality label.
fn volume_quality(volume_source: &str) -> &'static str {
    match volume_source.trim().to_uppercase().as_str() {
        "BINANCE_PAXG_PROXY" => "PAXG_PROXY",
        "BINANCE_PAXG" => "PAXG",
        "NONE" => "NONE",
        _ => "UNKNOWN",
    }
}

fn volume_warning(quality: &str) -> Option<&'static str> {
    matches!(quality, "PAXG" | "PAXG_PROXY").then_some(VOLUME_PROXY_WARNING)
}

/// Distance-to-trigger proximity bucket used by the Action View card.
fn proximity_label(distance_atr: Option<f64>) -> &'static str {
    match distance_atr {
        Some(value) if value.is_finite() && value <= 0.5 => "NEAR",
        Some(value) if value.is_finite() && value <= 1.5 => "MID",
        Some(value) if value.is_finite() => "FAR",
        _ => "UNKNOWN",
    }
}

fn json_number(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

/// Price/trigger/ATR distance fields for `/api/action-view`.
///
/// Current price priority: live signal entry → live-view last close → scenario
/// price. Trigger priority: PRIMARY trigger price → live signal entry. ATR is
/// the H4 ATR from the scenario market block.
fn proximity_fields(
    live_view: &Value,
    scenarios: &ScenarioResponse,
    signal: Option<&Signal>,
    primary: Option<&Scenario>,
) -> (Option<f64>, Option<f64>, &'static str) {
    let price = signal
        .map(|signal| signal.entry_price)
        .filter(|value| value.is_finite())
        .or_else(|| json_number(live_view, "last_close"))
        .or_else(|| scenarios.price.filter(|value| value.is_finite()));
    let trigger = primary
        .and_then(|scenario| scenario.trigger_price)
        .filter(|value| value.is_finite())
        .or_else(|| {
            signal
                .map(|signal| signal.entry_price)
                .filter(|value| value.is_finite())
        });
    let atr = scenarios
        .market
        .as_ref()
        .map(|market| market.h4_atr)
        .filter(|value| value.is_finite() && *value > 0.0);

    let distance_to_trigger = match (price, trigger) {
        (Some(price), Some(trigger)) => Some((price - trigger).abs()),
        _ => None,
    };
    let distance_atr = match (distance_to_trigger, atr) {
        (Some(distance), Some(atr)) => Some(distance / atr),
        _ => None,
    };

    (
        distance_to_trigger,
        distance_atr,
        proximity_label(distance_atr),
    )
}

fn generated_at(live_view: &Value) -> String {
    live_view
        .get("generated_at")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Utc::now().to_rfc3339())
}

/// Reuse the live-view macro/flow block when present; otherwise rebuild it from
/// the snapshot so the action view still has context on older/no-signal payloads.
fn macro_flow_view(
    live_view: &Value,
    signal_type: Option<&str>,
    macro_flow: Option<&MacroFlowSnapshot>,
) -> Option<Value> {
    if let Some(value) = live_view.get("macroFlow").filter(|value| !value.is_null()) {
        return Some(value.clone());
    }

    let snapshot = macro_flow?;
    let alignment = align_signal_with_macro_flow(signal_type.unwrap_or(""), Some(snapshot));
    Some(json!({
        "bias": alignment.bias,
        "confidence": alignment.confidence,
        "score": alignment.score,
        "flowQuality": alignment.flow_quality,
        "alignment": alignment.alignment,
        "note": alignment.note,
    }))
}

fn macro_alignment(
    live_view: &Value,
    signal_type: Option<&str>,
    macro_flow: Option<&MacroFlowSnapshot>,
) -> String {
    live_view
        .get("macroFlow")
        .and_then(|value| value.get("alignment"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            align_signal_with_macro_flow(signal_type.unwrap_or(""), macro_flow).alignment
        })
}

fn signal_from_live_view(live_view: &Value) -> Option<Signal> {
    live_view
        .get("signal")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

fn filter_text(filters: Option<&Value>, key: &str, fallback: &str) -> String {
    filters
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// Short Vietnamese reason listing every blocked display filter.
fn blocked_filter_reason(filters: Option<&Value>) -> String {
    let htf_ok = filters
        .and_then(|filters| filters.get("htf_ok"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rsi_ok = filters
        .and_then(|filters| filters.get("rsi_ok"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let delta_ok = filters
        .and_then(|filters| filters.get("delta_ok"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut blocked = Vec::new();
    if !htf_ok {
        blocked.push(format!(
            "HTF: {}",
            filter_text(filters, "htf_reason", "không đạt")
        ));
    }
    if !rsi_ok {
        blocked.push(format!(
            "RSI: {}",
            filter_text(filters, "rsi_reason", "không đạt")
        ));
    }
    if !delta_ok {
        blocked.push(format!(
            "Delta: {}",
            filter_text(filters, "delta_reason", "không đạt")
        ));
    }

    if blocked.is_empty() {
        "Setup đạt điều kiện kỹ thuật + filter.".to_string()
    } else {
        format!("Setup bị chặn bởi filter: {}.", blocked.join(" · "))
    }
}

fn primary_scenario(scenarios: &ScenarioResponse) -> Option<Scenario> {
    scenarios
        .scenarios
        .iter()
        .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"))
        .cloned()
}

/// Pure response builder for `/api/action-view` (no locks, no side effects).
///
/// It only reports what the read-only live view/scenario builders already
/// computed. The verdict never bypasses the existing scanner filters: a blocked
/// setup is reported as `WAIT`, not as an actionable entry.
pub(super) fn build_action_view(
    live_view: &Value,
    scenarios: &ScenarioResponse,
    macro_flow: Option<&MacroFlowSnapshot>,
    source: &str,
    volume_source: &str,
    trigger_alert_enabled: bool,
) -> Value {
    let quality = volume_quality(volume_source);
    let warning = volume_warning(quality);
    let signal = signal_from_live_view(live_view);
    let signal_type = signal
        .as_ref()
        .map(|signal| signal.signal_type.as_str())
        .filter(|_| live_view["status"] == "setup");
    let status = live_view
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("no_data");

    let mut verdict = "NO_TRADE";
    let mut headline = NO_TRADE_HEADLINE;
    let mut reason: String;
    let mut caution: Option<&str> = None;
    let mut primary: Option<Scenario> = None;

    match status {
        "setup" => {
            if let Some(signal) = signal.as_ref() {
                let filters = live_view.get("filters");
                let htf_ok = filters
                    .and_then(|filters| filters.get("htf_ok"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let rsi_ok = filters
                    .and_then(|filters| filters.get("rsi_ok"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let delta_ok = filters
                    .and_then(|filters| filters.get("delta_ok"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                if htf_ok && rsi_ok && delta_ok {
                    verdict = signal.signal_type.as_str();
                    headline = if verdict == "BUY" {
                        "Có thể vào BUY"
                    } else {
                        "Có thể vào SELL"
                    };
                    reason = "Setup đạt điều kiện kỹ thuật + filter.".to_string();
                    if macro_alignment(live_view, Some(verdict), macro_flow) == "COUNTER" {
                        caution = Some("COUNTER_MACRO");
                        reason.push_str(" Macro/Flow đang COUNTER — giảm size hoặc chờ xác nhận.");
                    }
                } else {
                    verdict = "WAIT";
                    headline = "Chờ xác nhận";
                    reason = blocked_filter_reason(filters);
                    caution = Some("FILTER_BLOCKED");
                }
            } else {
                reason = "Live view báo setup nhưng thiếu dữ liệu tín hiệu.".to_string();
            }
        }
        "no_setup" => {
            primary = primary_scenario(scenarios);
            if let Some(scenario) = primary.as_ref().filter(|scenario| scenario.valid) {
                verdict = "WAIT";
                headline = "Chờ trigger";
                reason = "Chưa có nến xác nhận; chờ trigger của kịch bản chính.".to_string();
                if scenario.macro_alignment.eq_ignore_ascii_case("COUNTER") {
                    caution = Some("COUNTER_MACRO");
                }
            } else {
                verdict = "NO_TRADE";
                headline = NO_TRADE_HEADLINE;
                reason = "Không có kịch bản valid; R:R/điều kiện chưa đạt.".to_string();
            }
        }
        _ => {
            verdict = "NO_TRADE";
            headline = "Chưa đủ dữ liệu";
            reason = live_view
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or(NO_DATA_FALLBACK)
                .to_string();
        }
    }

    // Volume quality is a data caveat, not a hard filter. Only surface it as the
    // primary caution when the view is actionable and no stronger caution applies.
    if caution.is_none()
        && matches!(verdict, "BUY" | "SELL")
        && matches!(quality, "PAXG" | "PAXG_PROXY")
    {
        caution = Some("VOLUME_PROXY");
    }

    let mut macro_flow_payload = macro_flow_view(live_view, signal_type, macro_flow);

    // When there is no confirmed signal, the live-view alignment is UNKNOWN.
    // Prefer the primary scenario's alignment so the action card is coherent
    // with its COUNTER_MACRO/ALIGNED caution.
    if let Some(primary) = primary.as_ref() {
        if let Some(flow) = macro_flow_payload.as_mut().and_then(Value::as_object_mut) {
            let is_unknown = flow
                .get("alignment")
                .and_then(Value::as_str)
                .map(|value| value.eq_ignore_ascii_case("UNKNOWN"))
                .unwrap_or(true);
            if is_unknown {
                let bias = flow
                    .get("bias")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN")
                    .to_string();
                let confidence = flow.get("confidence").and_then(Value::as_u64).unwrap_or(0);
                let flow_quality = flow
                    .get("flowQuality")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN")
                    .to_string();
                flow.insert("alignment".to_string(), json!(primary.macro_alignment));
                flow.insert(
                    "note".to_string(),
                    json!(format!(
                        "Macro {} ({}%) · flow {} — kịch bản chính {} là {}.",
                        bias, confidence, flow_quality, primary.name, primary.macro_alignment
                    )),
                );
            }
        }
    }

    let (distance_to_trigger, distance_atr, proximity) =
        proximity_fields(live_view, scenarios, signal.as_ref(), primary.as_ref());

    json!({
        "verdict": verdict,
        "headline": headline,
        "reason": reason,
        "caution": caution,
        "signal": signal,
        "primaryScenario": primary,
        "distanceToTrigger": distance_to_trigger,
        "distanceAtr": distance_atr,
        "proximity": proximity,
        "macroFlow": macro_flow_payload,
        "volumeQuality": quality,
        "volumeWarning": warning,
        "triggerAlertEnabled": trigger_alert_enabled,
        "source": source,
        "volumeSource": volume_source,
        "generated_at": generated_at(live_view),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macro_flow::MacroFlowScorecard;
    use crate::models::{EntryType, SignalStatus, SignalType, Timeframe};
    use crate::scenarios::ScenarioMarket;

    fn sample_signal(signal_type: SignalType) -> Signal {
        Signal {
            id: "SIG-TEST-ACTION".to_string(),
            symbol: "XAU/USD".to_string(),
            signal_type,
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
            macro_flow_alignment: None,
            macro_flow_note: None,
        }
    }

    fn setup_view(
        signal_type: SignalType,
        htf_ok: bool,
        rsi_ok: bool,
        delta_ok: bool,
        macro_alignment: Option<&str>,
    ) -> Value {
        let mut view = json!({
            "status": "setup",
            "signal": sample_signal(signal_type),
            "filters": {
                "htf_ok": htf_ok,
                "htf_reason": "H4 BEARISH chặn BUY",
                "rsi_ok": rsi_ok,
                "rsi_reason": "RSI quá mua",
                "delta_ok": delta_ok,
                "delta_reason": "Delta ngược hướng",
            },
            "source": "TWELVEDATA",
            "volumeSource": "BINANCE_PAXG_PROXY",
            "generated_at": "2026-11-12T00:00:00Z",
        });
        if let Some(alignment) = macro_alignment {
            view["macroFlow"] = json!({
                "bias": "BEARISH",
                "confidence": 95,
                "score": -4,
                "flowQuality": "CONFIRMING",
                "alignment": alignment,
                "note": "Macro BEARISH — BUY ngược bối cảnh vĩ mô.",
            });
        }
        view
    }

    fn no_setup_view() -> Value {
        json!({
            "status": "no_setup",
            "message": "Chưa đủ điều kiện Pinbar + Volume + vùng S/R.",
            "source": "TWELVEDATA",
            "volumeSource": "BINANCE_PAXG_PROXY",
            "generated_at": "2026-11-12T00:00:00Z",
        })
    }

    fn scenario(id: &str, role: &str, valid: bool, macro_alignment: &str) -> Scenario {
        Scenario {
            id: id.to_string(),
            name: "Bullish breakout".to_string(),
            bias: "BULLISH".to_string(),
            trigger: "Đóng M15 trên 2701.00".to_string(),
            condition: "Phá vỡ kháng cự gần nhất".to_string(),
            entry: Some(2701.00),
            stop_loss: Some(2691.00),
            take_profit_1: Some(2721.00),
            take_profit_2: Some(2731.00),
            risk_reward: Some(2.00),
            trigger_price: Some(2701.00),
            trigger_direction: "ABOVE".to_string(),
            trigger_timeframe: "M15".to_string(),
            invalidation: "Đóng M15 dưới 2690.00".to_string(),
            confidence: 80,
            macro_alignment: macro_alignment.to_string(),
            valid,
            role: role.to_string(),
            priority: 300,
            notes: Vec::new(),
        }
    }

    fn scenario_response(scenarios: Vec<Scenario>) -> ScenarioResponse {
        ScenarioResponse {
            status: "ok".to_string(),
            generated_at: "2026-11-12T00:00:00Z".to_string(),
            price: Some(2680.00),
            source: "TWELVEDATA".to_string(),
            volume_source: "BINANCE_PAXG_PROXY".to_string(),
            market: None,
            scenarios,
            warnings: Vec::new(),
        }
    }

    fn scenario_market(h4_atr: f64) -> ScenarioMarket {
        ScenarioMarket {
            h4_bias: "BEARISH".to_string(),
            h1_bias: "BEARISH".to_string(),
            d1_bias: "BEARISH".to_string(),
            h4_atr,
            nearest_support: Some(2670.0),
            nearest_resistance: Some(2700.0),
            rsi_m15: Some(45.0),
            rsi_state: "Neutral".to_string(),
            macro_bias: "BEARISH".to_string(),
            macro_confidence: 95,
        }
    }

    fn macro_snapshot(bias: &str, confidence: u8) -> MacroFlowSnapshot {
        MacroFlowSnapshot {
            updated_at: "2026-11-12T00:00:00Z".to_string(),
            fred_ok: true,
            yahoo_ok: true,
            cot_ok: true,
            metrics: Vec::new(),
            cot: None,
            scorecard: MacroFlowScorecard {
                macro_bias: bias.to_string(),
                confidence,
                score: 0,
                direction: "MIXED".to_string(),
                driver: "TEST".to_string(),
                flow_quality: "CONFIRMING".to_string(),
                risk_regime: "NEUTRAL".to_string(),
                rate_regime: "NEUTRAL".to_string(),
                usd_regime: "NEUTRAL".to_string(),
                vol_regime: "NORMAL".to_string(),
                notes: Vec::new(),
            },
            warnings: Vec::new(),
            sources: Vec::new(),
        }
    }

    fn build(
        live_view: &Value,
        scenarios: &ScenarioResponse,
        macro_flow: Option<&MacroFlowSnapshot>,
    ) -> Value {
        build_action_view(
            live_view,
            scenarios,
            macro_flow,
            "TWELVEDATA",
            "BINANCE_PAXG_PROXY",
            true,
        )
    }

    #[test]
    fn setup_with_all_filters_ok_returns_actionable_verdict() {
        let scenarios = scenario_response(vec![scenario(
            "bullish_breakout",
            "PRIMARY",
            true,
            "ALIGNED",
        )]);

        let buy = build(
            &setup_view(SignalType::Buy, true, true, true, None),
            &scenarios,
            None,
        );
        assert_eq!(buy["verdict"], "BUY");
        assert_eq!(buy["headline"], "Có thể vào BUY");
        assert_eq!(buy["reason"], "Setup đạt điều kiện kỹ thuật + filter.");
        assert_eq!(buy["signal"]["signal_type"], "BUY");
        assert!(buy["primaryScenario"].is_null());
        assert_eq!(buy["caution"], "VOLUME_PROXY");

        let sell = build(
            &setup_view(SignalType::Sell, true, true, true, None),
            &scenarios,
            None,
        );
        assert_eq!(sell["verdict"], "SELL");
        assert_eq!(sell["headline"], "Có thể vào SELL");
        assert_eq!(sell["signal"]["signal_type"], "SELL");
    }

    #[test]
    fn setup_with_blocked_filter_returns_wait_and_lists_reason() {
        let action = build(
            &setup_view(SignalType::Buy, false, true, true, None),
            &scenario_response(Vec::new()),
            None,
        );

        assert_eq!(action["verdict"], "WAIT");
        assert_eq!(action["headline"], "Chờ xác nhận");
        assert_eq!(action["caution"], "FILTER_BLOCKED");
        assert_eq!(action["signal"]["signal_type"], "BUY");
        assert!(action["primaryScenario"].is_null());
        assert!(action["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("HTF"));
    }

    #[test]
    fn no_setup_with_valid_primary_returns_wait_and_primary_context() {
        let primary = scenario("bullish_breakout", "PRIMARY", true, "ALIGNED");
        let action = build(&no_setup_view(), &scenario_response(vec![primary]), None);

        assert_eq!(action["verdict"], "WAIT");
        assert_eq!(action["headline"], "Chờ trigger");
        assert_eq!(
            action["reason"],
            "Chưa có nến xác nhận; chờ trigger của kịch bản chính."
        );
        assert_eq!(action["primaryScenario"]["id"], "bullish_breakout");
        assert_eq!(action["primaryScenario"]["valid"], true);
        assert!(action["signal"].is_null());
        assert!(action["caution"].is_null());
    }

    #[test]
    fn no_setup_with_invalid_primary_returns_no_trade_with_context() {
        let primary = scenario("bullish_breakout", "PRIMARY", false, "ALIGNED");
        let action = build(&no_setup_view(), &scenario_response(vec![primary]), None);

        assert_eq!(action["verdict"], "NO_TRADE");
        assert_eq!(action["headline"], "Đứng ngoài");
        assert_eq!(
            action["reason"],
            "Không có kịch bản valid; R:R/điều kiện chưa đạt."
        );
        assert_eq!(action["primaryScenario"]["id"], "bullish_breakout");
        assert_eq!(action["primaryScenario"]["valid"], false);
        assert!(action["signal"].is_null());
    }

    #[test]
    fn no_data_returns_no_trade_with_live_view_message() {
        let live_view = json!({
            "status": "no_data",
            "message": "Chưa đủ nến để phân tích.",
            "candles": 10,
        });
        let action = build(&live_view, &scenario_response(Vec::new()), None);

        assert_eq!(action["verdict"], "NO_TRADE");
        assert_eq!(action["headline"], "Chưa đủ dữ liệu");
        assert_eq!(action["reason"], "Chưa đủ nến để phân tích.");
        assert!(action["signal"].is_null());
        assert!(action["primaryScenario"].is_null());
    }

    #[test]
    fn counter_macro_adds_caution_without_blocking_the_signal() {
        let action = build(
            &setup_view(SignalType::Buy, true, true, true, Some("COUNTER")),
            &scenario_response(Vec::new()),
            None,
        );

        assert_eq!(action["verdict"], "BUY");
        assert_eq!(action["caution"], "COUNTER_MACRO");
        assert!(action["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("COUNTER"));
        assert_eq!(action["macroFlow"]["alignment"], "COUNTER");
    }

    #[test]
    fn counter_macro_primary_sets_caution_on_wait() {
        let primary = scenario("bearish_breakdown", "PRIMARY", true, "COUNTER");
        let action = build(&no_setup_view(), &scenario_response(vec![primary]), None);

        assert_eq!(action["verdict"], "WAIT");
        assert_eq!(action["caution"], "COUNTER_MACRO");
    }

    #[test]
    fn macro_flow_falls_back_to_snapshot_when_live_view_has_none() {
        let snapshot = macro_snapshot("BEARISH", 95);
        let action = build(
            &setup_view(SignalType::Buy, true, true, true, None),
            &scenario_response(Vec::new()),
            Some(&snapshot),
        );

        assert_eq!(action["macroFlow"]["bias"], "BEARISH");
        assert_eq!(action["macroFlow"]["confidence"], 95);
        assert_eq!(action["macroFlow"]["alignment"], "COUNTER");
        assert_eq!(action["caution"], "COUNTER_MACRO");
    }

    #[test]
    fn volume_quality_and_warning_map_sources() {
        assert_eq!(volume_quality("BINANCE_PAXG_PROXY"), "PAXG_PROXY");
        assert_eq!(volume_quality("BINANCE_PAXG"), "PAXG");
        assert_eq!(volume_quality("NONE"), "NONE");
        assert_eq!(volume_quality("TWELVEDATA"), "UNKNOWN");
        assert_eq!(volume_quality("binance_paxg"), "PAXG");
        assert_eq!(volume_warning("PAXG_PROXY"), Some(VOLUME_PROXY_WARNING));
        assert_eq!(volume_warning("PAXG"), Some(VOLUME_PROXY_WARNING));
        assert_eq!(volume_warning("NONE"), None);
        assert_eq!(volume_warning("UNKNOWN"), None);

        let action = build_action_view(
            &setup_view(SignalType::Buy, true, true, true, None),
            &scenario_response(Vec::new()),
            None,
            "TWELVEDATA",
            "BINANCE_PAXG",
            false,
        );
        assert_eq!(action["volumeQuality"], "PAXG");
        assert_eq!(action["volumeWarning"], VOLUME_PROXY_WARNING);
        assert_eq!(action["caution"], "VOLUME_PROXY");
    }

    #[test]
    fn proximity_label_uses_near_mid_far_thresholds() {
        assert_eq!(proximity_label(None), "UNKNOWN");
        assert_eq!(proximity_label(Some(0.0)), "NEAR");
        assert_eq!(proximity_label(Some(0.5)), "NEAR");
        assert_eq!(proximity_label(Some(0.5001)), "MID");
        assert_eq!(proximity_label(Some(1.5)), "MID");
        assert_eq!(proximity_label(Some(1.5001)), "FAR");
        assert_eq!(proximity_label(Some(f64::NAN)), "UNKNOWN");
        assert_eq!(proximity_label(Some(f64::INFINITY)), "UNKNOWN");
    }

    #[test]
    fn action_view_reports_distance_to_primary_trigger_in_atr_units() {
        let mut live_view = no_setup_view();
        live_view["last_close"] = json!(2721.00);

        let mut scenarios = scenario_response(vec![scenario(
            "bullish_breakout",
            "PRIMARY",
            true,
            "ALIGNED",
        )]);
        scenarios.market = Some(scenario_market(20.0));

        let action = build(&live_view, &scenarios, None);

        assert_eq!(action["distanceToTrigger"], 20.0);
        assert_eq!(action["distanceAtr"], 1.0);
        assert_eq!(action["proximity"], "MID");
    }

    #[test]
    fn action_view_distance_is_unknown_without_atr_or_trigger() {
        let mut live_view = no_setup_view();
        live_view["last_close"] = json!(2721.00);

        let scenarios = scenario_response(vec![scenario(
            "bullish_breakout",
            "PRIMARY",
            true,
            "ALIGNED",
        )]);
        let action = build(&live_view, &scenarios, None);
        assert_eq!(action["distanceToTrigger"], 20.0);
        assert!(action["distanceAtr"].is_null());
        assert_eq!(action["proximity"], "UNKNOWN");

        let mut without_trigger = scenario_response(vec![scenario(
            "bullish_breakout",
            "PRIMARY",
            true,
            "ALIGNED",
        )]);
        without_trigger.scenarios[0].trigger_price = None;
        without_trigger.scenarios[0].entry = None;
        without_trigger.market = Some(scenario_market(20.0));
        let action = build(&live_view, &without_trigger, None);
        assert!(action["distanceToTrigger"].is_null());
        assert!(action["distanceAtr"].is_null());
        assert_eq!(action["proximity"], "UNKNOWN");
    }

    #[test]
    fn setup_action_uses_signal_entry_as_trigger_distance_reference() {
        let mut scenarios = scenario_response(Vec::new());
        scenarios.market = Some(scenario_market(20.0));

        let action = build(
            &setup_view(SignalType::Buy, true, true, true, None),
            &scenarios,
            None,
        );

        assert_eq!(action["distanceToTrigger"], 0.0);
        assert_eq!(action["distanceAtr"], 0.0);
        assert_eq!(action["proximity"], "NEAR");
    }

    #[test]
    fn action_view_contains_the_documented_keys() {
        let action = build(
            &setup_view(SignalType::Buy, true, true, true, None),
            &scenario_response(Vec::new()),
            None,
        );

        for key in [
            "verdict",
            "headline",
            "reason",
            "caution",
            "signal",
            "primaryScenario",
            "distanceToTrigger",
            "distanceAtr",
            "proximity",
            "macroFlow",
            "volumeQuality",
            "volumeWarning",
            "triggerAlertEnabled",
            "source",
            "volumeSource",
            "generated_at",
        ] {
            assert!(action.get(key).is_some(), "missing action-view key: {key}");
        }
        assert_eq!(action["triggerAlertEnabled"], true);
        assert_eq!(action["source"], "TWELVEDATA");
        assert_eq!(action["volumeSource"], "BINANCE_PAXG_PROXY");
        assert!(action["generated_at"].as_str().is_some());
    }
}
