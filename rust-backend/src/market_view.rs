//! Market View / BotState layer (Phase 1).
//!
//! Separates the hard "no-trade" gates from the scoring/view layer. The Market
//! View is deterministic: it reuses the existing PRIMARY/ALTERNATE scenarios and
//! never invents prices. The AI strategist narrative can be layered on later
//! without changing these levels.

use crate::market_context::MarketContext;
use crate::market_strategist::StrategistView;
use crate::scenarios::{Scenario, ScenarioResponse};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// Three operating states. WATCHING is a sub-state of MARKET_VIEW.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BotState {
    NoTrade,
    MarketView,
    SniperTrigger,
}

impl BotState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoTrade => "NO_TRADE",
            Self::MarketView => "MARKET_VIEW",
            Self::SniperTrigger => "SNIPER_TRIGGER",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct H1Confirmation {
    /// CONFIRMED | PENDING | INVALID | UNKNOWN
    pub status: String,
    pub bias: String,
    pub close: Option<f64>,
    pub trigger_price: Option<f64>,
    pub atr14: Option<f64>,
    /// |H1 close - previous H1 close| / H1 ATR14.
    pub momentum_atr: Option<f64>,
    pub note: String,
    /// M15 retest zone for the advisory entry plan.
    pub m15_entry_zone_low: Option<f64>,
    pub m15_entry_zone_high: Option<f64>,
    pub m15_entry_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlookScenario {
    pub role: String,
    pub scenario_id: String,
    /// WATCH_SELL | WATCH_BUY | STAND_ASIDE
    pub action: String,
    /// Scenario passed level ordering, R:R and risk validation.
    #[serde(default)]
    pub valid: bool,
    pub zone_low: f64,
    pub zone_high: f64,
    pub trigger: String,
    pub stop_loss: Option<f64>,
    pub take_profit_1: Option<f64>,
    pub take_profit_2: Option<f64>,
    pub confidence: u8,
    pub invalidation: String,
    /// Advisory H1 confirmation + M15 retest plan. Never a signal by itself.
    #[serde(default)]
    pub h1_confirmation: Option<H1Confirmation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MarketScore {
    /// 0..=60 — D1/H4/H1 alignment with the PRIMARY bias.
    pub structure: u8,
    /// 0..=30 — RSI/volume/trigger-distance quality.
    pub trigger: u8,
    /// 0..=10 — macro alignment.
    pub macro_score: u8,
    /// 0..=100.
    pub total: u8,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketOutlook {
    pub bot_state: BotState,
    pub generated_at: String,
    pub valid_until: String,
    /// ACTIVE | TRIGGERED | RETEST_WINDOW | INVALIDATED | NO_TRADE
    #[serde(default)]
    pub plan_status: String,
    /// TREND_BULL | TREND_BEAR | TECHNICAL_PULLBACK | RANGE | REVERSAL_WATCH | MIXED | UNKNOWN
    pub structure: String,
    pub market_view: String,
    pub primary: Option<OutlookScenario>,
    pub alternative: Option<OutlookScenario>,
    pub risk_notes: Vec<String>,
    pub no_trade_reason: Option<String>,
    /// IN | OUT
    pub session: String,
    pub spread_proxy_atr: Option<f64>,
    pub d1_bias: String,
    pub h4_bias: String,
    pub h1_bias: String,
    pub price: Option<f64>,
    pub h4_atr: Option<f64>,
    /// Optional AI strategist narrative. Levels remain deterministic.
    pub strategist: Option<StrategistView>,
    /// Deterministic 60/30/10 score for the PRIMARY scenario.
    pub score: Option<MarketScore>,
}

pub struct OutlookInput<'a> {
    pub scenarios: &'a ScenarioResponse,
    pub context: Option<&'a MarketContext>,
    pub now: DateTime<Utc>,
    pub session_in: bool,
    pub no_trade_reason: Option<String>,
    pub news_notes: Vec<String>,
    pub spread_proxy_atr: Option<f64>,
    pub active_sniper: bool,
    pub strategist: Option<StrategistView>,
    pub score: Option<MarketScore>,
    pub plan_status: String,
}

pub fn build_outlook(input: OutlookInput<'_>) -> MarketOutlook {
    let context = input.context;
    let d1_bias = context
        .map(|context| context.d1.bias.to_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string());
    let h4_bias = context
        .map(|context| context.h4.bias.to_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string());
    let h1_bias = context
        .map(|context| context.h1.bias.to_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string());
    let h4_atr = context.map(|context| context.h4.atr14);
    let price = input
        .scenarios
        .price
        .or_else(|| context.map(|context| context.h1.close));
    let structure = classify_structure(context, input.scenarios);
    let primary = pick_primary(input.scenarios)
        .and_then(|scenario| outlook_scenario(scenario, h4_atr, context));
    let alternative = pick_alternative(input.scenarios)
        .and_then(|scenario| outlook_scenario(scenario, h4_atr, context));
    let bot_state = if input.no_trade_reason.is_some() {
        BotState::NoTrade
    } else if input.active_sniper {
        BotState::SniperTrigger
    } else {
        BotState::MarketView
    };
    let market_view = build_narrative(
        &structure,
        &d1_bias,
        &h4_bias,
        &h1_bias,
        price,
        context,
        primary.as_ref(),
    );
    let mut risk_notes = Vec::new();
    if let Some(reason) = input.no_trade_reason.as_ref() {
        risk_notes.push(reason.clone());
    }
    risk_notes.extend(input.news_notes);
    risk_notes.push(format!(
        "Phiên 13:00–23:00 VN: {}.",
        if input.session_in { "IN" } else { "OUT" }
    ));
    if let Some(spread) = input.spread_proxy_atr {
        risk_notes.push(format!("Spread proxy: {spread:.2}×H4 ATR."));
    }
    risk_notes.push("Kế hoạch tham khảo — chưa phải entry; không DCA ngược sóng.".to_string());

    MarketOutlook {
        bot_state,
        generated_at: input.now.to_rfc3339(),
        valid_until: next_daily_slot(input.now).to_rfc3339(),
        plan_status: input.plan_status,
        structure,
        market_view,
        primary,
        alternative,
        risk_notes,
        no_trade_reason: input.no_trade_reason,
        session: if input.session_in { "IN" } else { "OUT" }.to_string(),
        spread_proxy_atr: input.spread_proxy_atr,
        d1_bias,
        h4_bias,
        h1_bias,
        price,
        h4_atr,
        strategist: input.strategist,
        score: input.score,
    }
}

/// Deterministic 60/30/10 score for the PRIMARY scenario.
pub fn score_market(
    scenarios: &ScenarioResponse,
    context: Option<&MarketContext>,
    volume_ratio: Option<f64>,
) -> Option<MarketScore> {
    let primary = pick_primary(scenarios)?;
    Some(score_scenario(primary, context, volume_ratio))
}

pub fn score_scenario(
    scenario: &Scenario,
    context: Option<&MarketContext>,
    volume_ratio: Option<f64>,
) -> MarketScore {
    let bias = scenario.bias.trim().to_uppercase();
    let mut notes = Vec::new();
    let mut structure = 0u8;

    if let Some(context) = context {
        for (label, timeframe) in [
            ("D1", &context.d1),
            ("H4", &context.h4),
            ("H1", &context.h1),
        ] {
            let timeframe_bias = timeframe.bias.trim().to_uppercase();
            if timeframe_bias == bias {
                structure += 20;
            } else if timeframe_bias == "NEUTRAL" {
                structure += 10;
            }
            notes.push(format!("{label} bias: {timeframe_bias}."));
        }
    } else {
        notes.push("Structure: missing HTF context.".to_string());
    }

    let mut trigger = 0u8;
    let rsi = context
        .and_then(|context| context.rsi_m15.as_ref())
        .and_then(|rsi| rsi.value);
    match rsi {
        Some(value) if (30.0..=70.0).contains(&value) => {
            trigger += 10;
            notes.push(format!("RSI M15 {value:.1}: healthy."));
        }
        Some(value) => {
            trigger += 5;
            notes.push(format!("RSI M15 {value:.1}: extreme."));
        }
        None => notes.push("RSI M15: missing.".to_string()),
    }
    match volume_ratio {
        Some(ratio) if ratio >= 1.2 => {
            trigger += 10;
            notes.push(format!("Volume {ratio:.2}× MA20: expansion."));
        }
        Some(ratio) if ratio >= 1.0 => {
            trigger += 5;
            notes.push(format!("Volume {ratio:.2}× MA20: normal."));
        }
        Some(ratio) => notes.push(format!("Volume {ratio:.2}× MA20: low.")),
        None => notes.push("Volume: missing.".to_string()),
    }
    if let (Some(trigger_price), Some(context)) = (scenario.trigger_price, context) {
        let atr = context.h4.atr14.max(1.0);
        let distance = (context.h1.close - trigger_price).abs() / atr;
        if distance <= 0.5 {
            trigger += 10;
            notes.push(format!("Distance {distance:.2}×ATR: at trigger."));
        } else if distance <= 1.5 {
            trigger += 5;
            notes.push(format!("Distance {distance:.2}×ATR: near."));
        } else {
            notes.push(format!("Distance {distance:.2}×ATR: far."));
        }
    }

    let macro_score: u8 = match scenario.macro_alignment.trim().to_uppercase().as_str() {
        "ALIGNED" => 10,
        "MIXED" => 5,
        "COUNTER" => 0,
        _ => 3,
    };
    notes.push(format!("Macro: {}.", scenario.macro_alignment));
    let structure = structure.min(60);
    let trigger = trigger.min(30);
    let total = (structure + trigger + macro_score).min(100);
    MarketScore {
        structure,
        trigger,
        macro_score,
        total,
        notes,
    }
}

pub fn classify_structure(context: Option<&MarketContext>, scenarios: &ScenarioResponse) -> String {
    let Some(context) = context else {
        return "UNKNOWN".to_string();
    };
    let d1 = context.d1.bias.trim().to_uppercase();
    let h4 = context.h4.bias.trim().to_uppercase();
    let h1 = context.h1.bias.trim().to_uppercase();

    if d1 == h4 && h4 == h1 && matches!(d1.as_str(), "BULLISH" | "BEARISH") {
        return if d1 == "BULLISH" {
            "TREND_BULL".to_string()
        } else {
            "TREND_BEAR".to_string()
        };
    }

    if h4 == h1
        && h4 != d1
        && matches!(h4.as_str(), "BULLISH" | "BEARISH")
        && matches!(d1.as_str(), "BULLISH" | "BEARISH")
    {
        return "TECHNICAL_PULLBACK".to_string();
    }

    if d1 != h4
        && matches!(d1.as_str(), "BULLISH" | "BEARISH")
        && matches!(h4.as_str(), "BULLISH" | "BEARISH")
    {
        return "RANGE".to_string();
    }

    if let (Some(price), Some(support), Some(resistance)) = (
        scenarios.price.or(Some(context.h1.close)),
        context.h4.nearest_support,
        context.h4.nearest_resistance,
    ) {
        let atr = context.h4.atr14.max(1.0);
        let rsi = context.rsi_m15.as_ref().and_then(|rsi| rsi.value);
        let near_support = (price - support).abs() <= atr * 0.5;
        let near_resistance = (resistance - price).abs() <= atr * 0.5;
        if (near_support && rsi.is_some_and(|value| value <= 30.0))
            || (near_resistance && rsi.is_some_and(|value| value >= 70.0))
        {
            return "REVERSAL_WATCH".to_string();
        }
    }

    "MIXED".to_string()
}

fn pick_primary(scenarios: &ScenarioResponse) -> Option<&Scenario> {
    scenarios
        .scenarios
        .iter()
        .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"))
        .or_else(|| scenarios.scenarios.iter().find(|scenario| scenario.valid))
}

fn pick_alternative(scenarios: &ScenarioResponse) -> Option<&Scenario> {
    scenarios
        .scenarios
        .iter()
        .find(|scenario| scenario.role.eq_ignore_ascii_case("ALTERNATE"))
        .or_else(|| {
            scenarios
                .scenarios
                .iter()
                .find(|scenario| scenario.role.eq_ignore_ascii_case("WATCHLIST"))
        })
}

fn outlook_scenario(
    scenario: &Scenario,
    h4_atr: Option<f64>,
    context: Option<&MarketContext>,
) -> Option<OutlookScenario> {
    let anchor = scenario.entry.or(scenario.trigger_price)?;
    let buffer = (h4_atr.unwrap_or(10.0) * 0.10).max(1.5);
    let action = match scenario.bias.trim().to_uppercase().as_str() {
        "BEARISH" => "WATCH_SELL",
        "BULLISH" => "WATCH_BUY",
        _ => "STAND_ASIDE",
    };
    let zone_low = round2(anchor - buffer);
    let zone_high = round2(anchor + buffer);
    Some(OutlookScenario {
        role: scenario.role.trim().to_uppercase(),
        scenario_id: scenario.id.clone(),
        action: action.to_string(),
        valid: scenario.valid,
        zone_low,
        zone_high,
        trigger: scenario.trigger.clone(),
        stop_loss: scenario.stop_loss,
        take_profit_1: scenario.take_profit_1,
        take_profit_2: scenario.take_profit_2,
        confidence: scenario.confidence,
        invalidation: scenario.invalidation.clone(),
        h1_confirmation: Some(h1_confirmation(scenario, context, zone_low, zone_high)),
    })
}

/// Advisory H1 confirmation for an M15 scenario.
///
/// The H1 gate is intentionally simple and transparent:
/// - SELL breakdown: H1 close below trigger, H1 bias BEARISH, momentum >= 0.15 ATR.
/// - BUY breakout: mirror.
/// - TOUCH/limit: H1 confirmation is not applicable; wait for M15.
///
/// This never approves a signal; the live sniper still requires its M15 close +
/// confirmation + AI/hard risk.
fn h1_confirmation(
    scenario: &Scenario,
    context: Option<&MarketContext>,
    zone_low: f64,
    zone_high: f64,
) -> H1Confirmation {
    let Some(context) = context else {
        return H1Confirmation {
            status: "UNKNOWN".to_string(),
            bias: "UNKNOWN".to_string(),
            note: "Chưa có H1 context.".to_string(),
            m15_entry_zone_low: Some(zone_low),
            m15_entry_zone_high: Some(zone_high),
            m15_entry_note: "Chờ M15 retest vùng và xác nhận.".to_string(),
            ..H1Confirmation::default()
        };
    };
    let h1 = &context.h1;
    let h1_bias = h1.bias.trim().to_uppercase();
    let scenario_bias = scenario.bias.trim().to_uppercase();
    let direction = scenario.trigger_direction.trim().to_uppercase();
    let trigger = scenario.trigger_price;
    let atr = h1.atr14;
    let momentum_atr = if h1.prev_close > 0.0 && atr > 0.0 {
        Some((h1.close - h1.prev_close).abs() / atr)
    } else {
        None
    };
    let mut result = H1Confirmation {
        status: "UNKNOWN".to_string(),
        bias: h1_bias.clone(),
        close: Some(h1.close),
        trigger_price: trigger,
        atr14: Some(atr),
        momentum_atr,
        note: String::new(),
        m15_entry_zone_low: Some(zone_low),
        m15_entry_zone_high: Some(zone_high),
        m15_entry_note:
            "Chờ M15 retest vùng này và đóng cửa xác nhận; nếu không retest trong 4–8 nến M15, bỏ."
                .to_string(),
    };

    let Some(trigger) = trigger else {
        result.note = "Scenario không có trigger giá.".to_string();
        return result;
    };
    if scenario_bias == "BEARISH" && direction == "BELOW" {
        if h1_bias == "BULLISH" {
            result.status = "INVALID".to_string();
            result.note = "H1 bias BULLISH ngược kịch bản SELL.".to_string();
        } else if h1.close < trigger && h1_bias == "BEARISH" && momentum_atr.unwrap_or(0.0) >= 0.15
        {
            result.status = "CONFIRMED".to_string();
            result.note = format!(
                "H1 đóng {:.2} dưới trigger {:.2}, bias BEARISH, momentum {:.2}×ATR.",
                h1.close,
                trigger,
                momentum_atr.unwrap_or(0.0)
            );
        } else {
            result.status = "PENDING".to_string();
            result.note = format!(
                "H1 chưa xác nhận: close {:.2} vs trigger {:.2}, bias {}, momentum {}.",
                h1.close,
                trigger,
                h1_bias,
                momentum_atr
                    .map(|value| format!("{value:.2}×ATR"))
                    .unwrap_or_else(|| "n/a".to_string())
            );
        }
    } else if scenario_bias == "BULLISH" && direction == "ABOVE" {
        if h1_bias == "BEARISH" {
            result.status = "INVALID".to_string();
            result.note = "H1 bias BEARISH ngược kịch bản BUY.".to_string();
        } else if h1.close > trigger && h1_bias == "BULLISH" && momentum_atr.unwrap_or(0.0) >= 0.15
        {
            result.status = "CONFIRMED".to_string();
            result.note = format!(
                "H1 đóng {:.2} trên trigger {:.2}, bias BULLISH, momentum {:.2}×ATR.",
                h1.close,
                trigger,
                momentum_atr.unwrap_or(0.0)
            );
        } else {
            result.status = "PENDING".to_string();
            result.note = format!(
                "H1 chưa xác nhận: close {:.2} vs trigger {:.2}, bias {}, momentum {}.",
                h1.close,
                trigger,
                h1_bias,
                momentum_atr
                    .map(|value| format!("{value:.2}×ATR"))
                    .unwrap_or_else(|| "n/a".to_string())
            );
        }
    } else if direction == "TOUCH" {
        let opposite = (scenario_bias == "BULLISH" && h1_bias == "BEARISH")
            || (scenario_bias == "BEARISH" && h1_bias == "BULLISH");
        if opposite {
            result.status = "INVALID".to_string();
            result.note = format!(
                "Limit/touch ngược H1 bias {}; chỉ đánh bounce ngắn nếu M15 xác nhận mạnh.",
                h1_bias
            );
        } else {
            result.status = "PENDING".to_string();
            result.note = "Limit/touch: H1 confirmation không áp dụng; dùng M15 retest/rejection."
                .to_string();
        }
    } else {
        result.note = "Thiếu bias/trigger để xác nhận H1.".to_string();
    }
    result
}

fn build_narrative(
    structure: &str,
    d1: &str,
    h4: &str,
    h1: &str,
    price: Option<f64>,
    context: Option<&MarketContext>,
    primary: Option<&OutlookScenario>,
) -> String {
    let structure_vi = match structure {
        "TREND_BULL" => "Xu hướng tăng đồng thuận D1/H4/H1",
        "TREND_BEAR" => "Xu hướng giảm đồng thuận D1/H4/H1",
        "TECHNICAL_PULLBACK" => "Hồi kỹ thuật ngược xu hướng lớn",
        "RANGE" => "Range / hai chiều",
        "REVERSAL_WATCH" => "Cảnh báo đảo chiều tại key level",
        "MIXED" => "Cấu trúc hỗn hợp",
        _ => "Chưa đủ bối cảnh",
    };
    let price_text = price
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string());
    let support = context
        .and_then(|context| context.h4.nearest_support)
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string());
    let resistance = context
        .and_then(|context| context.h4.nearest_resistance)
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string());
    let primary_text = match primary {
        Some(primary) => format!(
            "Ưu tiên {} tại vùng {:.2}–{:.2}; SL {} / TP1 {} / TP2 {}.",
            action_vi(&primary.action),
            primary.zone_low,
            primary.zone_high,
            format_level(primary.stop_loss),
            format_level(primary.take_profit_1),
            format_level(primary.take_profit_2)
        ),
        None => "Chưa có PRIMARY hợp lệ — đứng ngoài, chờ cấu trúc mới.".to_string(),
    };
    format!(
        "{structure_vi}. D1 {d1} / H4 {h4} / H1 {h1}. Giá {price_text}; support {support} / resistance {resistance}. {primary_text}"
    )
}

fn action_vi(action: &str) -> &'static str {
    match action {
        "WATCH_SELL" => "canh Sell",
        "WATCH_BUY" => "canh Buy",
        _ => "đứng ngoài",
    }
}

fn format_level(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string())
}

/// Next Daily View slot in Vietnam time: 07:00, 13:00 or 19:00, weekdays.
fn next_daily_slot(now: DateTime<Utc>) -> DateTime<Utc> {
    const HOURS: [u32; 3] = [7, 13, 19];
    let vietnam_now = now + Duration::hours(7);
    for day_offset in 0..8 {
        let date = (vietnam_now + Duration::days(day_offset)).date_naive();
        if matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun) {
            continue;
        }
        for hour in HOURS {
            let Some(local) = date.and_hms_opt(hour, 0, 0) else {
                continue;
            };
            let candidate = Utc.from_utc_datetime(&local) - Duration::hours(7);
            if candidate > now {
                return candidate;
            }
        }
    }
    now + Duration::hours(1)
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_context::TimeframeContext;
    use crate::rsi::RsiSnapshot;
    use crate::scenarios::ScenarioMarket;
    use chrono::{TimeZone, Timelike};

    fn timeframe(bias: &str, close: f64, atr: f64) -> TimeframeContext {
        TimeframeContext {
            timeframe: "H4".to_string(),
            bias: bias.to_string(),
            close,
            prev_close: close,
            ema50: close,
            ema200: close,
            ema50_slope_3: 0.0,
            structure: "HH_HL".to_string(),
            channel_position: 0.5,
            atr14: atr,
            nearest_support: Some(close - 10.0),
            nearest_resistance: Some(close + 10.0),
            candles: 100,
            last_candle_timestamp: 0,
        }
    }

    fn context(d1: &str, h4: &str, h1: &str) -> MarketContext {
        MarketContext {
            d1: timeframe(d1, 2700.0, 20.0),
            h4: timeframe(h4, 2700.0, 12.0),
            h1: timeframe(h1, 2700.0, 8.0),
            updated_at: "2026-11-17T00:00:00Z".to_string(),
            price_source: "TEST".to_string(),
            spot_basis: None,
            rsi_m15: Some(RsiSnapshot {
                value: Some(50.0),
                state: "Neutral".to_string(),
                divergence: crate::rsi::RsiDivergence::None,
            }),
            rsi_h1: None,
            rsi_h4: None,
            rsi_d1: None,
        }
    }

    fn scenario(
        id: &str,
        role: &str,
        bias: &str,
        entry: f64,
        sl: f64,
        tp1: f64,
        tp2: f64,
    ) -> Scenario {
        Scenario {
            id: id.to_string(),
            name: id.to_string(),
            bias: bias.to_string(),
            trigger: "test trigger".to_string(),
            condition: "test".to_string(),
            entry: Some(entry),
            stop_loss: Some(sl),
            take_profit_1: Some(tp1),
            take_profit_2: Some(tp2),
            risk_reward: Some((tp1 - entry).abs() / (entry - sl).abs()),
            trigger_price: Some(entry),
            trigger_direction: if bias == "BEARISH" { "BELOW" } else { "ABOVE" }.to_string(),
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
                scenario(
                    "primary", "PRIMARY", "BEARISH", 2700.0, 2710.0, 2680.0, 2670.0,
                ),
                scenario(
                    "alt",
                    "ALTERNATE",
                    "BULLISH",
                    2710.0,
                    2700.0,
                    2730.0,
                    2740.0,
                ),
            ],
            warnings: vec![],
        }
    }

    #[test]
    fn bot_state_serializes_screaming_snake() {
        assert_eq!(
            serde_json::to_string(&BotState::NoTrade).expect("serialize"),
            "\"NO_TRADE\""
        );
        assert_eq!(
            serde_json::to_string(&BotState::MarketView).expect("serialize"),
            "\"MARKET_VIEW\""
        );
        assert_eq!(
            serde_json::to_string(&BotState::SniperTrigger).expect("serialize"),
            "\"SNIPER_TRIGGER\""
        );
    }

    #[test]
    fn classifies_trend_pullback_range_and_reversal() {
        let response = response();
        assert_eq!(
            classify_structure(Some(&context("BEARISH", "BEARISH", "BEARISH")), &response),
            "TREND_BEAR"
        );
        assert_eq!(
            classify_structure(Some(&context("BEARISH", "BULLISH", "BULLISH")), &response),
            "TECHNICAL_PULLBACK"
        );
        assert_eq!(
            classify_structure(Some(&context("BEARISH", "BULLISH", "BEARISH")), &response),
            "RANGE"
        );
    }

    #[test]
    fn score_components_follow_60_30_10() {
        let response = response();
        let context = context("BEARISH", "BEARISH", "BEARISH");
        let score = score_market(&response, Some(&context), Some(1.3)).expect("score");
        assert_eq!(score.structure, 60);
        assert_eq!(score.trigger, 30);
        assert_eq!(score.macro_score, 10);
        assert_eq!(score.total, 100);
    }

    #[test]
    fn h1_confirmation_marks_sell_breakdown() {
        let response = response();
        let primary = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "primary")
            .expect("primary");

        let mut confirmed_context = context("BEARISH", "BEARISH", "BEARISH");
        confirmed_context.h1.close = 2695.0;
        confirmed_context.h1.prev_close = 2705.0;
        let confirmed = h1_confirmation(primary, Some(&confirmed_context), 2698.0, 2702.0);
        assert_eq!(confirmed.status, "CONFIRMED");
        assert!(confirmed.momentum_atr.unwrap() > 0.15);

        let mut pending_context = context("BEARISH", "BEARISH", "BEARISH");
        pending_context.h1.close = 2705.0;
        pending_context.h1.prev_close = 2705.0;
        let pending = h1_confirmation(primary, Some(&pending_context), 2698.0, 2702.0);
        assert_eq!(pending.status, "PENDING");

        let mut invalid_context = context("BEARISH", "BEARISH", "BULLISH");
        invalid_context.h1.close = 2695.0;
        invalid_context.h1.prev_close = 2705.0;
        let invalid = h1_confirmation(primary, Some(&invalid_context), 2698.0, 2702.0);
        assert_eq!(invalid.status, "INVALID");
    }

    #[test]
    fn h1_confirmation_marks_buy_breakout() {
        let response = response();
        let alternative = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "alt")
            .expect("alternative");
        let mut bullish_context = context("BULLISH", "BULLISH", "BULLISH");
        bullish_context.h1.close = 2715.0;
        bullish_context.h1.prev_close = 2705.0;
        let confirmation = h1_confirmation(alternative, Some(&bullish_context), 2708.0, 2712.0);
        assert_eq!(confirmation.status, "CONFIRMED");
    }

    #[test]
    fn h1_confirmation_marks_touch_against_h1_bias_invalid() {
        let mut touch = scenario(
            "touch",
            "ALTERNATE",
            "BULLISH",
            2710.0,
            2700.0,
            2730.0,
            2740.0,
        );
        touch.trigger_direction = "TOUCH".to_string();
        let bearish_context = context("BEARISH", "BEARISH", "BEARISH");
        let confirmation = h1_confirmation(&touch, Some(&bearish_context), 2708.0, 2712.0);
        assert_eq!(confirmation.status, "INVALID");
        assert!(confirmation.note.contains("ngược H1 bias BEARISH"));
    }

    #[test]
    fn builds_market_view_with_primary_and_alternative_zones() {
        let response = response();
        let context = context("BEARISH", "BEARISH", "BEARISH");
        let outlook = build_outlook(OutlookInput {
            scenarios: &response,
            context: Some(&context),
            now: Utc.with_ymd_and_hms(2026, 11, 17, 10, 0, 0).unwrap(),
            session_in: false,
            no_trade_reason: None,
            news_notes: vec![],
            spread_proxy_atr: Some(0.2),
            active_sniper: false,
            strategist: None,
            score: None,
            plan_status: "ACTIVE".to_string(),
        });
        assert_eq!(outlook.bot_state, BotState::MarketView);
        assert_eq!(outlook.structure, "TREND_BEAR");
        let primary = outlook.primary.expect("primary");
        assert_eq!(primary.scenario_id, "primary");
        assert_eq!(primary.action, "WATCH_SELL");
        assert!(primary.zone_low < primary.zone_high);
        let alternative = outlook.alternative.expect("alternative");
        assert_eq!(alternative.scenario_id, "alt");
        assert_eq!(alternative.action, "WATCH_BUY");
        assert!(outlook.market_view.contains("support"));
    }

    #[test]
    fn no_trade_and_sniper_states_override_market_view() {
        let response = response();
        let context = context("BEARISH", "BEARISH", "BEARISH");
        let no_trade = build_outlook(OutlookInput {
            scenarios: &response,
            context: Some(&context),
            now: Utc.with_ymd_and_hms(2026, 11, 17, 10, 0, 0).unwrap(),
            session_in: false,
            no_trade_reason: Some("High-impact news ±30m".to_string()),
            news_notes: vec![],
            spread_proxy_atr: Some(1.2),
            active_sniper: false,
            strategist: None,
            score: None,
            plan_status: "ACTIVE".to_string(),
        });
        assert_eq!(no_trade.bot_state, BotState::NoTrade);
        assert!(no_trade.no_trade_reason.is_some());

        let sniper = build_outlook(OutlookInput {
            scenarios: &response,
            context: Some(&context),
            now: Utc.with_ymd_and_hms(2026, 11, 17, 10, 0, 0).unwrap(),
            session_in: true,
            no_trade_reason: None,
            news_notes: vec![],
            spread_proxy_atr: Some(0.2),
            active_sniper: true,
            strategist: None,
            score: None,
            plan_status: "ACTIVE".to_string(),
        });
        assert_eq!(sniper.bot_state, BotState::SniperTrigger);
    }

    #[test]
    fn valid_until_is_the_next_daily_slot() {
        let now = Utc.with_ymd_and_hms(2026, 11, 17, 10, 23, 0).unwrap(); // 17:23 VN
        let next = next_daily_slot(now);
        assert_eq!(next.hour(), 12); // 19:00 VN = 12:00 UTC
        assert_eq!(next.minute(), 0);
        assert_eq!(next.second(), 0);

        let late = Utc.with_ymd_and_hms(2026, 11, 17, 13, 0, 0).unwrap(); // 20:00 VN
        let next = next_daily_slot(late);
        assert_eq!(next.hour(), 0); // next day 07:00 VN = 00:00 UTC
    }
}
