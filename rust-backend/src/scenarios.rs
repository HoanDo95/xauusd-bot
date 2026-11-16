use crate::analyzer::GoldMarketAnalyzer;
use crate::macro_flow::{align_signal_with_macro_flow, MacroFlowSnapshot};
use crate::market_context::MarketContext;
use crate::models::{BotConfig, Candle, EntryType, SignalType, SupportResistanceZone};
use crate::rsi::{rsi_cross_sma, rsi_snapshot, RsiCross, RsiDivergence};
use crate::signal_validation::validate_signal_levels;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Cluster tolerance floor for the dynamic S/R zones used by the scenario view.
const SR_CLUSTER_TOLERANCE_MIN: f64 = 2.5;
/// ATR-scaled cluster tolerance for M15 zones (fraction of H4 ATR).
const SR_CLUSTER_ATR_FRACTION: f64 = 0.30;
/// ATR-scaled merge tolerance when combining M15 zones with HTF levels.
const HTF_MERGE_ATR_FRACTION: f64 = 0.50;
/// Prefer an HTF-confirmed level only when it is within this many ATR of price.
const HTF_PREFERRED_WINDOW_ATR: f64 = 3.0;
/// Proximity window for a limit setup at an HTF-confirmed level.
const HTF_PROXIMITY_ATR: f64 = 3.0;
/// Proximity window for a limit setup at a single-timeframe level.
const SINGLE_LEVEL_PROXIMITY_ATR: f64 = 1.5;
/// ATR floor so tiny M15 ranges do not collapse the scenario buffers.
const MIN_ATR: f64 = 2.5;
/// Minimum breakout buffer above/below a level.
const BREAKOUT_BUFFER_MIN: f64 = 2.0;
/// Minimum stop-loss buffer around a level.
const SL_BUFFER_MIN: f64 = 3.0;
/// Number of recent candles used for the swing-high/low fallback.
const SWING_FALLBACK_LOOKBACK: usize = 20;
/// Number of M15 candles used for the ATR fallback.
const ATR_PERIOD: usize = 14;
/// Confidence ceiling for reference scenarios.
const MAX_CONFIDENCE: i32 = 95;

/// One read-only reference scenario. Never an executable signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub name: String,
    /// BULLISH | BEARISH | NEUTRAL
    pub bias: String,
    /// Human-readable trigger condition.
    pub trigger: String,
    /// Short condition text.
    pub condition: String,
    pub entry: Option<f64>,
    pub stop_loss: Option<f64>,
    pub take_profit_1: Option<f64>,
    pub take_profit_2: Option<f64>,
    pub risk_reward: Option<f64>,
    /// Structured trigger price used by the read-only trigger alert worker.
    /// None when the scenario has no level to watch.
    #[serde(default)]
    pub trigger_price: Option<f64>,
    /// ABOVE | BELOW | TOUCH | NONE
    #[serde(default = "default_trigger_direction")]
    pub trigger_direction: String,
    /// Timeframe used to evaluate the trigger (M15).
    #[serde(default = "default_trigger_timeframe")]
    pub trigger_timeframe: String,
    /// Human-readable invalidation condition.
    pub invalidation: String,
    /// 0..=95
    pub confidence: u8,
    /// ALIGNED | COUNTER | MIXED | UNKNOWN
    pub macro_alignment: String,
    /// Levels ordered + R:R >= min + risk within range.
    pub valid: bool,
    /// PRIMARY | ALTERNATE | WATCHLIST — display ranking only.
    pub role: String,
    /// Transparent ranking score; higher is stronger.
    pub priority: i32,
    /// Vietnamese notes, including why the scenario is invalid.
    pub notes: Vec<String>,
}

fn default_trigger_direction() -> String {
    "NONE".to_string()
}

fn default_trigger_timeframe() -> String {
    "M15".to_string()
}

/// Evaluate a scenario trigger against one closed candle.
///
/// ABOVE/BELOW use the candle close (confirmation), while TOUCH uses the wick
/// range (the level was traded during the candle). Missing/NONE triggers never
/// fire. Equality is intentionally strict for ABOVE/BELOW and inclusive for
/// TOUCH.
pub fn scenario_triggered(scenario: &Scenario, candle: &Candle) -> bool {
    let Some(trigger_price) = scenario.trigger_price.filter(|value| value.is_finite()) else {
        return false;
    };

    match scenario.trigger_direction.trim().to_uppercase().as_str() {
        "ABOVE" => candle.close > trigger_price,
        "BELOW" => candle.close < trigger_price,
        "TOUCH" => candle.low <= trigger_price && candle.high >= trigger_price,
        _ => false,
    }
}

/// Read-only market snapshot that accompanies the reference scenarios.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioMarket {
    pub h4_bias: String,
    pub h1_bias: String,
    pub d1_bias: String,
    pub h4_atr: f64,
    pub nearest_support: Option<f64>,
    pub nearest_resistance: Option<f64>,
    pub rsi_m15: Option<f64>,
    pub rsi_state: String,
    pub macro_bias: String,
    pub macro_confidence: u8,
    /// Current-day change vs previous D1 close, in percent. None when unknown.
    #[serde(default)]
    pub daily_change_pct: Option<f64>,
    /// RSI H1 context, when available.
    #[serde(default)]
    pub rsi_h1: Option<f64>,
    /// RSI H4 context, when available.
    #[serde(default)]
    pub rsi_h4: Option<f64>,
    /// RSI D1 context, when available.
    #[serde(default)]
    pub rsi_d1: Option<f64>,
}

/// GET /api/scenarios response. Pure snapshot; no Telegram, quota or persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResponse {
    /// "ok" | "no_data"
    pub status: String,
    pub generated_at: String,
    pub price: Option<f64>,
    pub source: String,
    pub volume_source: String,
    pub market: Option<ScenarioMarket>,
    pub scenarios: Vec<Scenario>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Levels {
    entry: f64,
    stop_loss: f64,
    take_profit_1: f64,
    take_profit_2: f64,
}

impl Levels {
    fn new(entry: f64, stop_loss: f64, take_profit_1: f64, take_profit_2: f64) -> Self {
        Self {
            entry: round2(entry),
            stop_loss: round2(stop_loss),
            take_profit_1: round2(take_profit_1),
            take_profit_2: round2(take_profit_2),
        }
    }
}

struct ScenarioDraft {
    id: &'static str,
    name: &'static str,
    bias: &'static str,
    signal_type: SignalType,
    trigger: String,
    condition: String,
    invalidation: String,
    levels: Option<Levels>,
    trigger_price: Option<f64>,
    trigger_direction: &'static str,
    notes: Vec<String>,
}

struct ScenarioInputs<'a> {
    config: &'a BotConfig,
    price: f64,
    atr: f64,
    d1_bias: &'a str,
    h4_bias: &'a str,
    h1_bias: &'a str,
    macro_bias: &'a str,
    d1_structure: &'a str,
    h4_structure: &'a str,
    h1_structure: &'a str,
    h1_ema50_slope_3: f64,
    h4_ema50_slope_3: f64,
    volume_surge_ratio: f64,
    ema50_m15: f64,
    rsi_value: Option<f64>,
    rsi_h1: Option<f64>,
    rsi_h4: Option<f64>,
    rsi_m15_cross: RsiCross,
    m15_divergence: RsiDivergence,
    daily_change_pct: Option<f64>,
    macro_flow: Option<&'a MacroFlowSnapshot>,
}

/// Build four read-only reference scenarios for the current market.
///
/// Pure function: no network, no locks, no Telegram, no signal persistence and no
/// quota consumption. It intentionally does not call the strict signal engine, so
/// scenarios are available even when the scanner says `no_setup`.
// The builder mirrors the live-view signature and keeps every display input explicit.
#[allow(clippy::too_many_arguments)]
pub fn build_scenarios(
    candles: &[Candle],
    config: &BotConfig,
    live_price: Option<f64>,
    context: Option<&MarketContext>,
    macro_flow: Option<&MacroFlowSnapshot>,
    source: &str,
    volume_source: &str,
    now_ms: i64,
) -> ScenarioResponse {
    let generated_at = generated_at(now_ms);
    let warnings = build_warnings(context, macro_flow);
    let price = live_price
        .filter(|value| value.is_finite() && *value > 0.0)
        .or_else(|| {
            candles
                .last()
                .map(|candle| candle.close)
                .filter(|value| value.is_finite() && *value > 0.0)
        });

    if candles.len() < config.lookback_period.saturating_add(5) {
        return ScenarioResponse {
            status: "no_data".to_string(),
            generated_at,
            price: price.map(round2),
            source: source.to_string(),
            volume_source: volume_source.to_string(),
            market: None,
            scenarios: Vec::new(),
            warnings,
        };
    }

    let Some(price) = price else {
        return ScenarioResponse {
            status: "no_data".to_string(),
            generated_at,
            price: None,
            source: source.to_string(),
            volume_source: volume_source.to_string(),
            market: None,
            scenarios: Vec::new(),
            warnings,
        };
    };

    let atr = effective_atr(candles, context);
    let (support_set, resistance_set) = collect_levels(candles, config, price, atr, context);
    // Momentum setups use the confluence-preferred level; limit setups use the
    // nearest level so a bounce/rejection is not forced to wait 2-3 ATR.
    let nearest_support = support_set.nearest.map(|level| level.price);
    let nearest_resistance = resistance_set.nearest.map(|level| level.price);
    let bounce_support = support_set.nearest_overall.map(|level| level.price);
    let bounce_support_confluence = support_set
        .nearest_overall
        .map(|level| level.htf_confluence)
        .unwrap_or(0);
    let rejection_resistance = resistance_set.nearest_overall.map(|level| level.price);
    let rejection_resistance_confluence = resistance_set
        .nearest_overall
        .map(|level| level.htf_confluence)
        .unwrap_or(0);

    let breakout_buffer = (atr * 0.25).max(BREAKOUT_BUFFER_MIN);
    let sl_buffer = (atr * 0.50).max(SL_BUFFER_MIN);
    let swing_high = recent_swing_high(candles, STRUCTURE_LOOKBACK);
    let swing_low = recent_swing_low(candles, STRUCTURE_LOOKBACK);

    let rsi = rsi_snapshot(
        candles,
        config.rsi_period,
        config.rsi_overbought,
        config.rsi_oversold,
        config.rsi_divergence_lookback,
        config.rsi_pivot_k,
    );
    let (macro_bias, macro_confidence) = macro_view(macro_flow);
    let h4_bias = timeframe_bias(context.map(|context| &context.h4));
    let h1_bias = timeframe_bias(context.map(|context| &context.h1));
    let d1_bias = timeframe_bias(context.map(|context| &context.d1));
    let daily_change_pct = context.and_then(|context| {
        let prev_close = context.d1.prev_close;
        (prev_close.is_finite() && prev_close > 0.0)
            .then(|| round2((price - prev_close) / prev_close * 100.0))
    });

    let market = ScenarioMarket {
        h4_bias: h4_bias.clone(),
        h1_bias: h1_bias.clone(),
        d1_bias: d1_bias.clone(),
        h4_atr: round2(atr),
        nearest_support: nearest_support.map(round2),
        nearest_resistance: nearest_resistance.map(round2),
        rsi_m15: rsi.value.map(round2),
        rsi_state: rsi.state,
        macro_bias: macro_bias.clone(),
        macro_confidence,
        daily_change_pct,
        rsi_h1: context
            .and_then(|context| context.rsi_h1.as_ref())
            .and_then(|snapshot| snapshot.value),
        rsi_h4: context
            .and_then(|context| context.rsi_h4.as_ref())
            .and_then(|snapshot| snapshot.value),
        rsi_d1: context
            .and_then(|context| context.rsi_d1.as_ref())
            .and_then(|snapshot| snapshot.value),
    };

    let inputs = ScenarioInputs {
        config,
        price,
        atr,
        d1_bias: &d1_bias,
        h4_bias: &h4_bias,
        h1_bias: &h1_bias,
        macro_bias: &macro_bias,
        d1_structure: context
            .map(|context| context.d1.structure.as_str())
            .unwrap_or("UNKNOWN"),
        h4_structure: context
            .map(|context| context.h4.structure.as_str())
            .unwrap_or("UNKNOWN"),
        h1_structure: context
            .map(|context| context.h1.structure.as_str())
            .unwrap_or("UNKNOWN"),
        h1_ema50_slope_3: context
            .map(|context| context.h1.ema50_slope_3)
            .unwrap_or(0.0),
        h4_ema50_slope_3: context
            .map(|context| context.h4.ema50_slope_3)
            .unwrap_or(0.0),
        volume_surge_ratio: volume_surge_ratio(candles, config.volume_ma_period),
        ema50_m15: GoldMarketAnalyzer::calculate_ema(candles, 50),
        rsi_value: rsi.value,
        rsi_h1: context
            .and_then(|context| context.rsi_h1.as_ref())
            .and_then(|snapshot| snapshot.value),
        rsi_h4: context
            .and_then(|context| context.rsi_h4.as_ref())
            .and_then(|snapshot| snapshot.value),
        rsi_m15_cross: rsi_cross_sma(candles, config.rsi_period, 14),
        m15_divergence: rsi.divergence,
        daily_change_pct,
        macro_flow,
    };

    let drafts = vec![
        bullish_breakout_draft(
            price,
            atr,
            breakout_buffer,
            sl_buffer,
            nearest_resistance,
            &resistance_set.levels,
            config.min_risk_reward,
            swing_low,
            config.structural_sl_atr_cap,
        ),
        bearish_breakdown_draft(
            price,
            atr,
            breakout_buffer,
            sl_buffer,
            nearest_support,
            &support_set.levels,
            config.min_risk_reward,
            swing_high,
            config.structural_sl_atr_cap,
        ),
        bullish_bounce_draft(
            price,
            atr,
            breakout_buffer,
            sl_buffer,
            bounce_support,
            nearest_resistance,
            bounce_support_confluence,
            config.min_risk_reward,
        ),
        bearish_rejection_draft(
            price,
            atr,
            breakout_buffer,
            sl_buffer,
            nearest_support,
            rejection_resistance,
            rejection_resistance_confluence,
            config.min_risk_reward,
        ),
    ];

    let mut scenarios: Vec<Scenario> = drafts
        .into_iter()
        .map(|draft| finalize_scenario(draft, &inputs))
        .collect();
    apply_anti_chase_filter(&mut scenarios, &inputs);
    apply_strategy_filters(&mut scenarios, &inputs);
    let scenarios = rank_scenarios(scenarios);

    ScenarioResponse {
        status: "ok".to_string(),
        generated_at,
        price: Some(round2(price)),
        source: source.to_string(),
        volume_source: volume_source.to_string(),
        market: Some(market),
        scenarios,
        warnings,
    }
}

/// Minimum distance from entry to TP1 so TP1 satisfies the configured R:R.
/// At least 2R is always required; a one-cent buffer keeps the target valid
/// after price rounding.
fn min_tp1_reward(risk: f64, min_risk_reward: f64) -> f64 {
    let min_rr = if min_risk_reward.is_finite() && min_risk_reward > 0.0 {
        min_risk_reward
    } else {
        2.0
    };
    risk * min_rr.max(2.0) + 0.01
}

/// Number of closed M15 candles used for the structural swing anchor.
const STRUCTURE_LOOKBACK: usize = 20;

fn recent_swing_high(candles: &[Candle], lookback: usize) -> Option<f64> {
    if candles.len() < 2 {
        return None;
    }
    let end = candles.len() - 1;
    let start = end.saturating_sub(lookback);
    candles[start..end]
        .iter()
        .map(|candle| candle.high)
        .filter(|value| value.is_finite())
        .reduce(f64::max)
}

fn recent_swing_low(candles: &[Candle], lookback: usize) -> Option<f64> {
    if candles.len() < 2 {
        return None;
    }
    let end = candles.len() - 1;
    let start = end.saturating_sub(lookback);
    candles[start..end]
        .iter()
        .map(|candle| candle.low)
        .filter(|value| value.is_finite())
        .reduce(f64::min)
}

fn structural_buffer(atr: f64) -> f64 {
    (atr * 0.10).max(3.0)
}

#[allow(clippy::too_many_arguments)]
fn bullish_breakout_draft(
    price: f64,
    atr: f64,
    breakout_buffer: f64,
    sl_buffer: f64,
    nearest_resistance: Option<f64>,
    resistance_levels: &[f64],
    min_risk_reward: f64,
    swing_low: Option<f64>,
    sl_cap_atr: f64,
) -> ScenarioDraft {
    let trigger = round2(
        nearest_resistance
            .map(|level| level + breakout_buffer)
            .unwrap_or(price + atr),
    );
    let atr_stop_loss = round2(
        nearest_resistance
            .map(|level| level - sl_buffer)
            .unwrap_or(trigger - 2.0 * atr),
    );
    let cap = nearest_resistance
        .filter(|_| sl_cap_atr > 0.0)
        .map(|level| level - atr * sl_cap_atr);
    let stop_loss = swing_low
        .map(|low| round2(low - structural_buffer(atr)))
        .map(|structural| {
            let capped = cap.map_or(structural, |cap| structural.max(cap));
            round2(capped.min(atr_stop_loss))
        })
        .unwrap_or(atr_stop_loss);
    let risk = (trigger - stop_loss).abs();
    let tp1_reward = min_tp1_reward(risk, min_risk_reward);
    let take_profit_1 = round2(
        resistance_levels
            .iter()
            .copied()
            .find(|level| *level >= trigger + tp1_reward)
            .unwrap_or(trigger + tp1_reward),
    );
    let take_profit_2 = round2(
        resistance_levels
            .iter()
            .copied()
            .find(|level| *level > take_profit_1)
            .unwrap_or_else(|| (trigger + 3.0 * risk).max(take_profit_1 + 1.0)),
    );

    let invalidation = match nearest_resistance {
        Some(level) => format!("Đóng M15 trở lại dưới {:.2}", level),
        None => format!("Đóng M15 trở lại dưới {:.2}", trigger),
    };
    let trigger_text = match nearest_resistance {
        Some(level) => format!("Đóng M15 trên {:.2} (phá kháng cự {:.2})", trigger, level),
        None => format!("Đóng M15 trên {:.2}", trigger),
    };

    ScenarioDraft {
        id: "bullish_breakout",
        name: "Bullish breakout",
        bias: "BULLISH",
        signal_type: SignalType::Buy,
        trigger: trigger_text,
        condition: "Phá vỡ kháng cự gần nhất".to_string(),
        invalidation,
        levels: Some(Levels::new(
            trigger,
            stop_loss,
            take_profit_1,
            take_profit_2,
        )),
        trigger_price: nearest_resistance.map(|_| trigger),
        trigger_direction: if nearest_resistance.is_some() {
            "ABOVE"
        } else {
            "NONE"
        },
        notes: vec!["Kịch bản breakout cần nến M15 đóng cửa xác nhận.".to_string()],
    }
}

#[allow(clippy::too_many_arguments)]
fn bearish_breakdown_draft(
    price: f64,
    atr: f64,
    breakout_buffer: f64,
    sl_buffer: f64,
    nearest_support: Option<f64>,
    support_levels: &[f64],
    min_risk_reward: f64,
    swing_high: Option<f64>,
    sl_cap_atr: f64,
) -> ScenarioDraft {
    let trigger = round2(
        nearest_support
            .map(|level| level - breakout_buffer)
            .unwrap_or(price - atr),
    );
    let atr_stop_loss = round2(
        nearest_support
            .map(|level| level + sl_buffer)
            .unwrap_or(trigger + 2.0 * atr),
    );
    let cap = nearest_support
        .filter(|_| sl_cap_atr > 0.0)
        .map(|level| level + atr * sl_cap_atr);
    let stop_loss = swing_high
        .map(|high| round2(high + structural_buffer(atr)))
        .map(|structural| {
            let capped = cap.map_or(structural, |cap| structural.min(cap));
            round2(capped.max(atr_stop_loss))
        })
        .unwrap_or(atr_stop_loss);
    let risk = (stop_loss - trigger).abs();
    let tp1_reward = min_tp1_reward(risk, min_risk_reward);
    let take_profit_1 = round2(
        support_levels
            .iter()
            .copied()
            .find(|level| *level <= trigger - tp1_reward)
            .unwrap_or(trigger - tp1_reward),
    );
    let take_profit_2 = round2(
        support_levels
            .iter()
            .copied()
            .find(|level| *level < take_profit_1)
            .unwrap_or_else(|| (trigger - 3.0 * risk).min(take_profit_1 - 1.0)),
    );

    let invalidation = match nearest_support {
        Some(level) => format!("Đóng M15 trở lại trên {:.2}", level),
        None => format!("Đóng M15 trở lại trên {:.2}", trigger),
    };
    let trigger_text = match nearest_support {
        Some(level) => format!("Đóng M15 dưới {:.2} (phá hỗ trợ {:.2})", trigger, level),
        None => format!("Đóng M15 dưới {:.2}", trigger),
    };

    ScenarioDraft {
        id: "bearish_breakdown",
        name: "Bearish breakdown",
        bias: "BEARISH",
        signal_type: SignalType::Sell,
        trigger: trigger_text,
        condition: "Phá vỡ hỗ trợ gần nhất".to_string(),
        invalidation,
        levels: Some(Levels::new(
            trigger,
            stop_loss,
            take_profit_1,
            take_profit_2,
        )),
        trigger_price: nearest_support.map(|_| trigger),
        trigger_direction: if nearest_support.is_some() {
            "BELOW"
        } else {
            "NONE"
        },
        notes: vec!["Kịch bản breakdown cần nến M15 đóng cửa xác nhận.".to_string()],
    }
}

#[allow(clippy::too_many_arguments)]
fn bullish_bounce_draft(
    price: f64,
    atr: f64,
    breakout_buffer: f64,
    sl_buffer: f64,
    nearest_support: Option<f64>,
    nearest_resistance: Option<f64>,
    support_confluence: u8,
    min_risk_reward: f64,
) -> ScenarioDraft {
    let Some(support) = nearest_support else {
        return ScenarioDraft {
            id: "bullish_bounce",
            name: "Bullish bounce",
            bias: "BULLISH",
            signal_type: SignalType::Buy,
            trigger: "Chưa có vùng hỗ trợ gần để chờ bounce.".to_string(),
            condition: "Bounce từ hỗ trợ gần nhất".to_string(),
            invalidation: "Không áp dụng — thiếu vùng hỗ trợ.".to_string(),
            levels: None,
            trigger_price: None,
            trigger_direction: "NONE",
            notes: vec!["Không có vùng hỗ trợ gần.".to_string()],
        };
    };

    let distance = (price - support).abs();
    let max_distance = if support_confluence >= 2 {
        atr * HTF_PROXIMITY_ATR
    } else {
        atr * SINGLE_LEVEL_PROXIMITY_ATR
    };
    if distance > max_distance {
        return ScenarioDraft {
            id: "bullish_bounce",
            name: "Bullish bounce",
            bias: "BULLISH",
            signal_type: SignalType::Buy,
            trigger: format!(
                "Hỗ trợ {:.2} cách giá {:.2} — chưa đủ gần để bounce.",
                support, distance
            ),
            condition: "Bounce từ hỗ trợ gần nhất".to_string(),
            invalidation: format!("Đóng M15 dưới {:.2}", support),
            levels: None,
            trigger_price: None,
            trigger_direction: "NONE",
            notes: vec![format!(
                "Hỗ trợ gần nhất quá xa ({:.2} > {:.1}×ATR = {:.2}) — không đủ điều kiện.",
                distance,
                max_distance / atr,
                max_distance
            )],
        };
    }

    let entry = round2(support + breakout_buffer);
    let stop_loss = round2(support - sl_buffer);
    let risk = (entry - stop_loss).abs();
    let tp1_reward = min_tp1_reward(risk, min_risk_reward);
    let take_profit_1 = round2(match nearest_resistance {
        Some(resistance) if (support + resistance) / 2.0 >= entry + tp1_reward => {
            (support + resistance) / 2.0
        }
        _ => entry + tp1_reward,
    });
    let take_profit_2 = round2(match nearest_resistance {
        Some(resistance) if resistance > take_profit_1 => resistance,
        _ => (entry + 3.0 * risk).max(take_profit_1 + 1.0),
    });

    ScenarioDraft {
        id: "bullish_bounce",
        name: "Bullish bounce",
        bias: "BULLISH",
        signal_type: SignalType::Buy,
        trigger: format!("Chạm hỗ trợ {:.2} và đóng M15 trên {:.2}", support, entry),
        condition: "Bounce từ hỗ trợ gần nhất".to_string(),
        invalidation: format!("Đóng M15 dưới {:.2}", support),
        levels: Some(Levels::new(entry, stop_loss, take_profit_1, take_profit_2)),
        trigger_price: Some(round2(support)),
        trigger_direction: "TOUCH",
        notes: vec!["Kịch bản bounce cần nến M15 giữ được vùng hỗ trợ.".to_string()],
    }
}

#[allow(clippy::too_many_arguments)]
fn bearish_rejection_draft(
    price: f64,
    atr: f64,
    breakout_buffer: f64,
    sl_buffer: f64,
    nearest_support: Option<f64>,
    nearest_resistance: Option<f64>,
    resistance_confluence: u8,
    min_risk_reward: f64,
) -> ScenarioDraft {
    let Some(resistance) = nearest_resistance else {
        return ScenarioDraft {
            id: "bearish_rejection",
            name: "Bearish rejection",
            bias: "BEARISH",
            signal_type: SignalType::Sell,
            trigger: "Chưa có vùng kháng cự gần để chờ rejection.".to_string(),
            condition: "Rejection tại kháng cự gần nhất".to_string(),
            invalidation: "Không áp dụng — thiếu vùng kháng cự.".to_string(),
            levels: None,
            trigger_price: None,
            trigger_direction: "NONE",
            notes: vec!["Không có vùng kháng cự gần.".to_string()],
        };
    };

    let distance = (resistance - price).abs();
    let max_distance = if resistance_confluence >= 2 {
        atr * HTF_PROXIMITY_ATR
    } else {
        atr * SINGLE_LEVEL_PROXIMITY_ATR
    };
    if distance > max_distance {
        return ScenarioDraft {
            id: "bearish_rejection",
            name: "Bearish rejection",
            bias: "BEARISH",
            signal_type: SignalType::Sell,
            trigger: format!(
                "Kháng cự {:.2} cách giá {:.2} — chưa đủ gần để rejection.",
                resistance, distance
            ),
            condition: "Rejection tại kháng cự gần nhất".to_string(),
            invalidation: format!("Đóng M15 trên {:.2}", resistance),
            levels: None,
            trigger_price: None,
            trigger_direction: "NONE",
            notes: vec![format!(
                "Kháng cự gần nhất quá xa ({:.2} > {:.1}×ATR = {:.2}) — không đủ điều kiện.",
                distance,
                max_distance / atr,
                max_distance
            )],
        };
    }

    let entry = round2(resistance - breakout_buffer);
    let stop_loss = round2(resistance + sl_buffer);
    let risk = (stop_loss - entry).abs();
    let tp1_reward = min_tp1_reward(risk, min_risk_reward);
    let take_profit_1 = round2(match nearest_support {
        Some(support) if (support + resistance) / 2.0 <= entry - tp1_reward => {
            (support + resistance) / 2.0
        }
        _ => entry - tp1_reward,
    });
    let take_profit_2 = round2(match nearest_support {
        Some(support) if support < take_profit_1 => support,
        _ => (entry - 3.0 * risk).min(take_profit_1 - 1.0),
    });

    ScenarioDraft {
        id: "bearish_rejection",
        name: "Bearish rejection",
        bias: "BEARISH",
        signal_type: SignalType::Sell,
        trigger: format!(
            "Chạm kháng cự {:.2} và đóng M15 dưới {:.2}",
            resistance, entry
        ),
        condition: "Rejection tại kháng cự gần nhất".to_string(),
        invalidation: format!("Đóng M15 trên {:.2}", resistance),
        levels: Some(Levels::new(entry, stop_loss, take_profit_1, take_profit_2)),
        trigger_price: Some(round2(resistance)),
        trigger_direction: "TOUCH",
        notes: vec!["Kịch bản rejection cần nến M15 giữ được vùng kháng cự.".to_string()],
    }
}

fn finalize_scenario(draft: ScenarioDraft, inputs: &ScenarioInputs<'_>) -> Scenario {
    let mut notes = draft.notes;
    let macro_alignment = scenario_macro_alignment(draft.bias, inputs.macro_flow);

    let (entry, stop_loss, take_profit_1, take_profit_2, risk_reward, valid) = match draft.levels {
        Some(levels) => {
            let risk = (levels.entry - levels.stop_loss).abs();
            let risk_reward = if risk > 0.0 {
                Some((levels.take_profit_1 - levels.entry).abs() / risk)
            } else {
                None
            };
            let valid = match validate_signal_levels(
                draft.signal_type,
                levels.entry,
                levels.stop_loss,
                levels.take_profit_1,
                levels.take_profit_2,
                inputs.config.min_risk_reward,
            ) {
                Ok(()) => true,
                Err(reason) => {
                    notes.push(format!("Không hợp lệ: {reason}"));
                    false
                }
            };
            if let Some(risk_reward) =
                risk_reward.filter(|value| *value + 1e-9 < inputs.config.min_risk_reward)
            {
                notes.push(format!(
                    "R:R {:.2} < min {:.2} — không đủ điều kiện.",
                    risk_reward, inputs.config.min_risk_reward
                ));
            }
            (
                Some(levels.entry),
                Some(levels.stop_loss),
                Some(levels.take_profit_1),
                Some(levels.take_profit_2),
                risk_reward,
                valid,
            )
        }
        None => (None, None, None, None, None, false),
    };

    if macro_alignment == "COUNTER" {
        notes.push(format!(
            "Macro {} — {} là counter-macro.",
            inputs.macro_bias,
            draft.signal_type.as_str()
        ));
    }

    let confidence = scenario_confidence(
        draft.id,
        draft.bias,
        draft.signal_type,
        entry,
        risk_reward,
        inputs,
        &macro_alignment,
    );
    let priority = scenario_priority(
        confidence,
        valid,
        &macro_alignment,
        draft.bias,
        risk_reward,
        entry,
        inputs,
    );

    Scenario {
        id: draft.id.to_string(),
        name: draft.name.to_string(),
        bias: draft.bias.to_string(),
        trigger: draft.trigger,
        condition: draft.condition,
        entry,
        stop_loss,
        take_profit_1,
        take_profit_2,
        risk_reward,
        trigger_price: draft.trigger_price.map(round2),
        trigger_direction: draft.trigger_direction.to_string(),
        trigger_timeframe: "M15".to_string(),
        invalidation: draft.invalidation,
        confidence,
        macro_alignment,
        valid,
        role: "WATCHLIST".to_string(),
        priority,
        notes,
    }
}

/// Transparent display ranking. It never gates a signal, quota or Telegram send;
/// it only decides which reference scenario is shown first.
fn scenario_priority(
    confidence: u8,
    valid: bool,
    macro_alignment: &str,
    bias: &str,
    risk_reward: Option<f64>,
    entry: Option<f64>,
    inputs: &ScenarioInputs<'_>,
) -> i32 {
    let mut priority = i32::from(confidence);
    if valid {
        // A viable scenario must outrank an invalid one; macro/H4 alignment
        // can only break ties among viable scenarios.
        priority += 200;
    }
    match macro_alignment {
        "ALIGNED" => priority += 30,
        "COUNTER" => priority -= 30,
        _ => {}
    }
    let directional_bias = matches!(bias, "BULLISH" | "BEARISH");
    if directional_bias && bias == inputs.h4_bias {
        priority += 20;
    }
    if risk_reward.is_some_and(|value| value + 1e-9 >= inputs.config.min_risk_reward) {
        priority += 10;
    }
    if entry.is_some_and(|value| (inputs.price - value).abs() <= inputs.atr * 1.5) {
        priority += 10;
    }
    priority
}

/// Sort viable scenarios first, then by priority/confidence, then label one
/// PRIMARY, one opposite-bias ALTERNATE and the rest WATCHLIST. The returned
/// order is PRIMARY, ALTERNATE, then the remaining WATCHLIST by priority.
fn rank_scenarios(mut scenarios: Vec<Scenario>) -> Vec<Scenario> {
    scenarios.sort_by(|left, right| {
        right
            .valid
            .cmp(&left.valid)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| right.confidence.cmp(&left.confidence))
            .then_with(|| left.id.cmp(&right.id))
    });

    if scenarios.is_empty() {
        return scenarios;
    }

    // Backtest 2025: COUNTER-macro PRIMARY trades averaged -0.50R and were
    // statistically negative. Prefer an aligned scenario for PRIMARY; if none
    // exists, keep the counter scenario visible but mark it non-tradable.
    if scenarios[0].macro_alignment.eq_ignore_ascii_case("COUNTER") {
        if let Some(aligned_index) = scenarios
            .iter()
            .position(|scenario| scenario.macro_alignment.eq_ignore_ascii_case("ALIGNED"))
        {
            let aligned = scenarios.remove(aligned_index);
            scenarios.insert(0, aligned);
        } else {
            scenarios[0].valid = false;
            scenarios[0]
                .notes
                .push("COUNTER macro — không có setup aligned, không vào lệnh.".to_string());
        }
    }

    let primary_bias = scenarios[0].bias.clone();
    let alternate_index = scenarios
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, scenario)| biases_are_opposite(&primary_bias, &scenario.bias))
        .map(|(index, _)| index)
        .or_else(|| (scenarios.len() > 1).then_some(1));

    for (index, scenario) in scenarios.iter_mut().enumerate() {
        scenario.role = if index == 0 {
            "PRIMARY"
        } else if Some(index) == alternate_index {
            "ALTERNATE"
        } else {
            "WATCHLIST"
        }
        .to_string();
    }

    // Keep ALTERNATE directly after PRIMARY even when a higher-priority
    // WATCHLIST scenario originally sat between them.
    if let Some(alternate_index) = alternate_index {
        let alternate = scenarios.remove(alternate_index);
        scenarios.insert(1, alternate);
    }

    scenarios
}

fn biases_are_opposite(left: &str, right: &str) -> bool {
    matches!(
        (left, right),
        ("BULLISH", "BEARISH") | ("BEARISH", "BULLISH")
    )
}

fn anti_chase_active(inputs: &ScenarioInputs<'_>, bias: &str) -> bool {
    if !inputs.config.anti_chase_filter_enabled {
        return false;
    }
    let Some(daily_change_pct) = inputs.daily_change_pct else {
        return false;
    };
    let rsi_h1_extreme = inputs.rsi_h1.is_some_and(|value| match bias {
        "BEARISH" => value < inputs.config.anti_chase_rsi_h1,
        "BULLISH" => value > 100.0 - inputs.config.anti_chase_rsi_h1,
        _ => false,
    });
    match bias {
        "BEARISH" => {
            daily_change_pct <= inputs.config.anti_chase_daily_change_pct
                && (rsi_h1_extreme || inputs.m15_divergence == RsiDivergence::Bullish)
        }
        "BULLISH" => {
            daily_change_pct >= -inputs.config.anti_chase_daily_change_pct
                && (rsi_h1_extreme || inputs.m15_divergence == RsiDivergence::Bearish)
        }
        _ => false,
    }
}

/// Deterministic gate used after AI approval: block a BREAKOUT entry when the
/// market has already exhausted its daily range and RSI H1 / M15 divergence
/// warns against chasing. Returns the human-readable reason when blocked.
pub fn anti_chase_blocks_breakout(
    signal_type: SignalType,
    entry_type: EntryType,
    daily_change_pct: Option<f64>,
    rsi_h1: Option<f64>,
    m15_divergence: RsiDivergence,
    config: &BotConfig,
) -> Option<String> {
    if !config.anti_chase_filter_enabled || entry_type != EntryType::Breakout {
        return None;
    }
    let change = daily_change_pct?;
    let (bias, extreme, opposite_divergence, change_triggered) = match signal_type {
        SignalType::Sell => (
            "BEARISH",
            rsi_h1.is_some_and(|value| value < config.anti_chase_rsi_h1),
            m15_divergence == RsiDivergence::Bullish,
            change <= config.anti_chase_daily_change_pct,
        ),
        SignalType::Buy => (
            "BULLISH",
            rsi_h1.is_some_and(|value| value > 100.0 - config.anti_chase_rsi_h1),
            m15_divergence == RsiDivergence::Bearish,
            change >= -config.anti_chase_daily_change_pct,
        ),
    };
    if !(change_triggered && (extreme || opposite_divergence)) {
        return None;
    }
    Some(format!(
        "Anti-chase: {bias} breakout bị chặn (daily change {change:.2}%, RSI H1 {}, M15 divergence {}).",
        rsi_h1
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "n/a".to_string()),
        match m15_divergence {
            RsiDivergence::Bullish => "Bullish",
            RsiDivergence::Bearish => "Bearish",
            RsiDivergence::None => "None",
        }
    ))
}

fn apply_strategy_filters(scenarios: &mut [Scenario], inputs: &ScenarioInputs<'_>) {
    if inputs.config.bearish_rejection_enabled {
        return;
    }
    for scenario in scenarios.iter_mut() {
        if scenario.id == "bearish_rejection" {
            scenario.valid = false;
            scenario.priority = scenario.priority.min(0);
            scenario.notes.push(
                "Backtest 2025: bearish_rejection -0.57R/trade — đang tắt; bật bearish_rejection_enabled để tham khảo."
                    .to_string(),
            );
        }
    }
}

fn apply_anti_chase_filter(scenarios: &mut [Scenario], inputs: &ScenarioInputs<'_>) {
    for scenario in scenarios.iter_mut() {
        if !matches!(
            scenario.id.as_str(),
            "bullish_breakout" | "bearish_breakdown"
        ) || !anti_chase_active(inputs, &scenario.bias)
        {
            continue;
        }
        scenario.valid = false;
        scenario.confidence = scenario.confidence.min(65);
        scenario.priority = scenario.priority.min(65);
        scenario.notes.push(format!(
            "Anti-chase: daily change {:.2}% · RSI H1 {} · M15 divergence {} — tắt breakout, ưu tiên pullback.",
            inputs.daily_change_pct.unwrap_or(0.0),
            inputs
                .rsi_h1
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "n/a".to_string()),
            match inputs.m15_divergence {
                RsiDivergence::Bullish => "Bullish",
                RsiDivergence::Bearish => "Bearish",
                RsiDivergence::None => "None",
            }
        ));
    }
}

fn pullback_confirmation(
    scenario_id: &str,
    signal_type: SignalType,
    inputs: &ScenarioInputs<'_>,
) -> bool {
    let rsi_h1_healthy = inputs
        .rsi_h1
        .is_some_and(|value| (30.0..=65.0).contains(&value));
    if !rsi_h1_healthy {
        return false;
    }
    match signal_type {
        SignalType::Sell => {
            scenario_id == "bearish_rejection"
                && inputs
                    .rsi_value
                    .is_some_and(|value| value >= inputs.config.rsi_overbought)
        }
        SignalType::Buy => {
            scenario_id == "bullish_bounce"
                && inputs
                    .rsi_value
                    .is_some_and(|value| value <= inputs.config.rsi_oversold)
        }
    }
}

/// Weighted confluence v2 (quantum 30/25/20/15/10).
///
/// Feature-flagged: only used when `quantum_weighted_score_enabled=true`.
/// Keeps the same 90+ confidence caps as v1 so early-entry semantics do not
/// silently loosen.
fn scenario_confidence_v2(
    bias: &str,
    signal_type: SignalType,
    entry: Option<f64>,
    risk_reward: Option<f64>,
    inputs: &ScenarioInputs<'_>,
    macro_alignment: &str,
) -> u8 {
    let mut score: i32 = 0;

    // 1) Trend alignment: 30 points.
    let mut trend = 0;
    if bias == inputs.d1_bias {
        trend += 7;
    }
    if bias == inputs.h4_bias {
        trend += 8;
    }
    if bias == inputs.h1_bias {
        trend += 7;
    }
    if bias == inputs.macro_bias {
        trend += 8;
    }
    let slope_ok = match signal_type {
        SignalType::Buy => inputs.h4_ema50_slope_3 > 0.0 && inputs.h1_ema50_slope_3 > 0.0,
        SignalType::Sell => inputs.h4_ema50_slope_3 < 0.0 && inputs.h1_ema50_slope_3 < 0.0,
    };
    if slope_ok {
        trend += 5;
    }
    score += trend.min(30);

    // 2) Key level reaction + swing structure: 25 points.
    let mut level = 0;
    if let Some(entry) = entry {
        let distance_atr = (inputs.price - entry).abs() / inputs.atr.max(1e-9);
        level += match distance_atr {
            value if value <= 0.5 => 15,
            value if value <= 1.0 => 12,
            value if value <= 1.5 => 8,
            _ => 3,
        };
    }
    let structure_points = match signal_type {
        SignalType::Buy => {
            [
                inputs.d1_structure,
                inputs.h4_structure,
                inputs.h1_structure,
            ]
            .iter()
            .filter(|structure| **structure == "HH_HL")
            .count() as i32
                * 3
        }
        SignalType::Sell => {
            [
                inputs.d1_structure,
                inputs.h4_structure,
                inputs.h1_structure,
            ]
            .iter()
            .filter(|structure| **structure == "LH_LL")
            .count() as i32
                * 3
        }
    };
    level += structure_points.min(10);
    score += level.min(25);

    // 3) Volume confirmation: 20 points.
    let volume_high = inputs.config.quantum_volume_confirm_ratio.max(1.0);
    score += if inputs.volume_surge_ratio >= volume_high {
        20
    } else if inputs.volume_surge_ratio >= 1.0 {
        12
    } else if inputs.volume_surge_ratio >= 0.8 {
        6
    } else {
        3
    };

    // 4) RSI momentum: 15 points (H1 range + M15 RSI/SMA14 cross).
    let rsi_points = match (signal_type, inputs.rsi_h1) {
        (SignalType::Sell, Some(value)) if (35.0..=55.0).contains(&value) => 10,
        (SignalType::Buy, Some(value)) if (45.0..=65.0).contains(&value) => 10,
        (_, Some(value)) if (30.0..=65.0).contains(&value) => 7,
        (_, Some(_)) => 2,
        (_, None) => 5,
    };
    let cross_points = match (signal_type, inputs.rsi_m15_cross) {
        (SignalType::Sell, RsiCross::Bearish) | (SignalType::Buy, RsiCross::Bullish) => 5,
        _ => 0,
    };
    score += (rsi_points + cross_points).min(15);

    // 5) Macro / order flow: 10 points.
    score += match macro_alignment {
        "ALIGNED" => 10,
        "MIXED" => 5,
        _ => 0,
    };

    if risk_reward.is_some_and(|value| value + 1e-9 < inputs.config.min_risk_reward) {
        score = score.min(60);
    }

    // Anti-chase: price far from EMA50 M15 is a late entry.
    if inputs.ema50_m15 > 0.0 {
        let distance_atr = (inputs.price - inputs.ema50_m15).abs() / inputs.atr.max(1e-9);
        if distance_atr > 1.5 {
            score = score.min(75);
        }
    }

    let macro_aligned =
        bias == inputs.macro_bias && bias == inputs.d1_bias && bias == inputs.h4_bias;
    let rsi_h1_healthy = inputs
        .rsi_h1
        .is_some_and(|value| (30.0..=65.0).contains(&value));
    let rsi_h4_extreme = inputs
        .rsi_h4
        .is_some_and(|value| !(30.0..=70.0).contains(&value));
    let opposite_divergence = match signal_type {
        SignalType::Buy => inputs.m15_divergence == RsiDivergence::Bearish,
        SignalType::Sell => inputs.m15_divergence == RsiDivergence::Bullish,
    };
    let mut cap = MAX_CONFIDENCE;
    if !macro_aligned {
        cap = cap.min(85);
    }
    if !rsi_h1_healthy {
        cap = cap.min(85);
    }
    if rsi_h4_extreme {
        cap = cap.min(85);
    }
    if opposite_divergence {
        cap = cap.min(75);
    }
    if (!rsi_h1_healthy || rsi_h4_extreme) && opposite_divergence {
        cap = cap.min(65);
    }
    score = score.min(cap);

    score.clamp(0, MAX_CONFIDENCE) as u8
}

fn scenario_confidence(
    scenario_id: &str,
    bias: &str,
    signal_type: SignalType,
    entry: Option<f64>,
    risk_reward: Option<f64>,
    inputs: &ScenarioInputs<'_>,
    macro_alignment: &str,
) -> u8 {
    if inputs.config.quantum_weighted_score_enabled {
        return scenario_confidence_v2(
            bias,
            signal_type,
            entry,
            risk_reward,
            inputs,
            macro_alignment,
        );
    }

    let mut score: i32 = 50;

    if bias == inputs.h4_bias {
        score += 10;
    }
    if bias == inputs.macro_bias {
        score += 10;
    }
    if let Some(rsi) = inputs.rsi_value {
        let not_extreme = match signal_type {
            SignalType::Buy => rsi <= inputs.config.rsi_overbought,
            SignalType::Sell => rsi >= inputs.config.rsi_oversold,
        };
        if not_extreme {
            score += 10;
        }
    }
    if risk_reward.is_some_and(|value| value + 1e-9 >= inputs.config.min_risk_reward) {
        score += 10;
    }
    if entry.is_some_and(|value| (inputs.price - value).abs() <= inputs.atr * 1.5) {
        score += 5;
    }
    let pullback_confirmed = pullback_confirmation(scenario_id, signal_type, inputs);
    if pullback_confirmed {
        // RSI M15 overbought at resistance / oversold at support confirms the
        // pullback exhaustion. This is a bonus, not a free pass: macro and
        // RSI H1 health are still required below.
        score += 10;
    }

    match macro_alignment {
        "COUNTER" => score -= 15,
        "ALIGNED" => score += 5,
        _ => {}
    }

    // Confidence >= 90 is reserved for full agreement: macro + D1/H4 bias,
    // healthy RSI H1, and no opposite M15 divergence. This is a deterministic
    // cap, not an AI suggestion.
    let macro_aligned =
        bias == inputs.macro_bias && bias == inputs.d1_bias && bias == inputs.h4_bias;
    let rsi_h1_healthy = inputs
        .rsi_h1
        .is_some_and(|value| (30.0..=65.0).contains(&value));
    let rsi_h4_extreme = inputs
        .rsi_h4
        .is_some_and(|value| !(30.0..=70.0).contains(&value));
    let opposite_divergence = match signal_type {
        SignalType::Buy => inputs.m15_divergence == RsiDivergence::Bearish,
        SignalType::Sell => inputs.m15_divergence == RsiDivergence::Bullish,
    };
    let mut cap = MAX_CONFIDENCE;
    if !macro_aligned {
        cap = cap.min(85);
    }
    if !pullback_confirmed {
        if inputs.rsi_h1.is_some() && !rsi_h1_healthy {
            cap = cap.min(85);
        }
        if rsi_h4_extreme {
            cap = cap.min(85);
        }
        if opposite_divergence {
            cap = cap.min(75);
        }
        if (inputs.rsi_h1.is_some() && !rsi_h1_healthy || rsi_h4_extreme) && opposite_divergence {
            cap = cap.min(65);
        }
    }
    score = score.min(cap);

    score.clamp(0, MAX_CONFIDENCE) as u8
}

fn scenario_macro_alignment(bias: &str, macro_flow: Option<&MacroFlowSnapshot>) -> String {
    let signal_type = match bias {
        "BULLISH" => "BUY",
        "BEARISH" => "SELL",
        _ => "",
    };
    align_signal_with_macro_flow(signal_type, macro_flow).alignment
}

/// One merged S/R level. `htf_confluence` counts how many distinct HTF
/// timeframes (H1/H4/D1) confirm it; M15 zones do not add confluence.
#[derive(Debug, Clone, Copy)]
struct LevelPick {
    price: f64,
    htf_confluence: u8,
}

/// Merged level set for one side of price.
struct LevelSet {
    /// All merged prices, nearest first (supports descending, resistances ascending).
    levels: Vec<f64>,
    /// Confluence-preferred level used for breakout/breakdown triggers.
    nearest: Option<LevelPick>,
    /// Nearest level regardless of confluence, used for bounce/rejection limits.
    nearest_overall: Option<LevelPick>,
}

const SOURCE_M15: u8 = 1 << 0;
const SOURCE_H1: u8 = 1 << 1;
const SOURCE_H4: u8 = 1 << 2;
const SOURCE_D1: u8 = 1 << 3;

#[derive(Debug, Clone, Copy)]
struct LevelCandidate {
    price: f64,
    mask: u8,
    priority: u8,
}

fn collect_levels(
    candles: &[Candle],
    config: &BotConfig,
    price: f64,
    atr: f64,
    context: Option<&MarketContext>,
) -> (LevelSet, LevelSet) {
    let m15_tolerance = if config.htf_levels_enabled {
        (atr * SR_CLUSTER_ATR_FRACTION).max(SR_CLUSTER_TOLERANCE_MIN)
    } else {
        SR_CLUSTER_TOLERANCE_MIN
    };
    let (zones_support, zones_resistance) =
        GoldMarketAnalyzer::detect_dynamic_sr_zones(candles, config.lookback_period, m15_tolerance);

    if !config.htf_levels_enabled {
        return legacy_collect_levels(candles, &zones_support, &zones_resistance, price, context);
    }

    let mut candidates: Vec<LevelCandidate> = zones_support
        .iter()
        .chain(zones_resistance.iter())
        .filter(|zone| zone.price.is_finite() && zone.price > 0.0)
        .map(|zone| LevelCandidate {
            price: zone.price,
            mask: SOURCE_M15,
            priority: 0,
        })
        .collect();

    if let Some(context) = context {
        for (timeframe, mask, priority) in [
            (&context.h1, SOURCE_H1, 1u8),
            (&context.h4, SOURCE_H4, 2u8),
            (&context.d1, SOURCE_D1, 3u8),
        ] {
            for level in [
                timeframe.nearest_support,
                timeframe.nearest_resistance,
                Some(timeframe.ema50),
                Some(timeframe.ema200),
            ]
            .into_iter()
            .flatten()
            {
                if level.is_finite() && level > 0.0 {
                    candidates.push(LevelCandidate {
                        price: level,
                        mask,
                        priority,
                    });
                }
            }
        }
    }

    let merge_tolerance = (atr * HTF_MERGE_ATR_FRACTION).max(SR_CLUSTER_TOLERANCE_MIN);
    let mut merged = merge_level_candidates(candidates, merge_tolerance);

    let start = candles.len().saturating_sub(SWING_FALLBACK_LOOKBACK);
    if !merged.iter().any(|level| level.price < price) {
        if let Some(level) = candles[start..]
            .iter()
            .map(|candle| candle.low)
            .filter(|level| level.is_finite() && *level < price)
            .max_by(f64::total_cmp)
        {
            merged.push(LevelPick {
                price: level,
                htf_confluence: 0,
            });
        }
    }
    if !merged.iter().any(|level| level.price > price) {
        if let Some(level) = candles[start..]
            .iter()
            .map(|candle| candle.high)
            .filter(|level| level.is_finite() && *level > price)
            .min_by(f64::total_cmp)
        {
            merged.push(LevelPick {
                price: level,
                htf_confluence: 0,
            });
        }
    }

    let mut supports: Vec<LevelPick> = merged
        .iter()
        .copied()
        .filter(|level| level.price < price)
        .collect();
    supports.sort_by(|left, right| right.price.total_cmp(&left.price));

    let mut resistances: Vec<LevelPick> = merged
        .iter()
        .copied()
        .filter(|level| level.price > price)
        .collect();
    resistances.sort_by(|left, right| left.price.total_cmp(&right.price));

    (
        build_level_set(&supports, price, atr),
        build_level_set(&resistances, price, atr),
    )
}

fn legacy_collect_levels(
    candles: &[Candle],
    zones_support: &[SupportResistanceZone],
    zones_resistance: &[SupportResistanceZone],
    price: f64,
    context: Option<&MarketContext>,
) -> (LevelSet, LevelSet) {
    let mut support_levels: Vec<f64> = zones_support
        .iter()
        .map(|zone| zone.price)
        .filter(|level| level.is_finite() && *level < price)
        .collect();
    support_levels.sort_by(|left, right| right.total_cmp(left));

    let mut resistance_levels: Vec<f64> = zones_resistance
        .iter()
        .map(|zone| zone.price)
        .filter(|level| level.is_finite() && *level > price)
        .collect();
    resistance_levels.sort_by(f64::total_cmp);

    if support_levels.is_empty() {
        let mut fallback = Vec::new();
        if let Some(context) = context {
            for level in [
                context.h4.nearest_support,
                context.h1.nearest_support,
                context.d1.nearest_support,
            ]
            .into_iter()
            .flatten()
            {
                if level.is_finite() && level < price {
                    fallback.push(level);
                }
            }
        }
        let start = candles.len().saturating_sub(SWING_FALLBACK_LOOKBACK);
        if let Some(level) = candles[start..]
            .iter()
            .map(|candle| candle.low)
            .filter(|level| level.is_finite() && *level < price)
            .max_by(f64::total_cmp)
        {
            fallback.push(level);
        }
        fallback.sort_by(|left, right| right.total_cmp(left));
        support_levels = fallback;
    }

    if resistance_levels.is_empty() {
        let mut fallback = Vec::new();
        if let Some(context) = context {
            for level in [
                context.h4.nearest_resistance,
                context.h1.nearest_resistance,
                context.d1.nearest_resistance,
            ]
            .into_iter()
            .flatten()
            {
                if level.is_finite() && level > price {
                    fallback.push(level);
                }
            }
        }
        let start = candles.len().saturating_sub(SWING_FALLBACK_LOOKBACK);
        if let Some(level) = candles[start..]
            .iter()
            .map(|candle| candle.high)
            .filter(|level| level.is_finite() && *level > price)
            .min_by(f64::total_cmp)
        {
            fallback.push(level);
        }
        fallback.sort_by(f64::total_cmp);
        resistance_levels = fallback;
    }

    let support_nearest = support_levels.first().copied().map(|price| LevelPick {
        price,
        htf_confluence: 0,
    });
    let resistance_nearest = resistance_levels.first().copied().map(|price| LevelPick {
        price,
        htf_confluence: 0,
    });

    (
        LevelSet {
            levels: support_levels,
            nearest: support_nearest,
            nearest_overall: support_nearest,
        },
        LevelSet {
            levels: resistance_levels,
            nearest: resistance_nearest,
            nearest_overall: resistance_nearest,
        },
    )
}

fn merge_level_candidates(mut candidates: Vec<LevelCandidate>, tolerance: f64) -> Vec<LevelPick> {
    candidates.sort_by(|left, right| left.price.total_cmp(&right.price));
    let mut clusters: Vec<Vec<LevelCandidate>> = Vec::new();
    for candidate in candidates {
        match clusters.last_mut() {
            Some(cluster) if (candidate.price - cluster[0].price).abs() <= tolerance => {
                cluster.push(candidate);
            }
            _ => clusters.push(vec![candidate]),
        }
    }

    clusters
        .into_iter()
        .map(|cluster| {
            let mask = cluster.iter().fold(0u8, |mask, level| mask | level.mask);
            let best = cluster
                .iter()
                .max_by_key(|level| level.priority)
                .copied()
                .expect("cluster is non-empty");
            LevelPick {
                price: best.price,
                htf_confluence: (mask & !SOURCE_M15).count_ones() as u8,
            }
        })
        .collect()
}

fn build_level_set(levels: &[LevelPick], price: f64, atr: f64) -> LevelSet {
    let window = atr * HTF_PREFERRED_WINDOW_ATR;
    let in_window = |level: &LevelPick| (level.price - price).abs() <= window;
    let nearest = levels
        .iter()
        .copied()
        .find(|level| level.htf_confluence >= 2 && in_window(level))
        .or_else(|| {
            levels
                .iter()
                .copied()
                .find(|level| level.htf_confluence >= 1 && in_window(level))
        })
        .or_else(|| levels.first().copied());

    LevelSet {
        levels: levels.iter().map(|level| level.price).collect(),
        nearest,
        nearest_overall: levels.first().copied(),
    }
}

/// Latest volume vs the average of the previous `period` candles.
fn volume_surge_ratio(candles: &[Candle], period: usize) -> f64 {
    let period = period.max(1);
    if candles.len() <= period {
        return 0.0;
    }
    let latest = candles.last().map(|candle| candle.volume).unwrap_or(0.0);
    let start = candles.len() - 1 - period;
    let average = candles[start..candles.len() - 1]
        .iter()
        .map(|candle| candle.volume)
        .sum::<f64>()
        / period as f64;
    if average > 0.0 && latest.is_finite() && latest >= 0.0 {
        latest / average
    } else {
        0.0
    }
}

fn effective_atr(candles: &[Candle], context: Option<&MarketContext>) -> f64 {
    let context_atr = context
        .map(|context| {
            if context.h4.atr14 > 0.0 {
                context.h4.atr14
            } else {
                context.h1.atr14
            }
        })
        .unwrap_or(0.0);

    let atr = if context_atr.is_finite() && context_atr > 0.0 {
        context_atr
    } else {
        let start = candles.len().saturating_sub(ATR_PERIOD);
        let slice = &candles[start..];
        if slice.is_empty() {
            0.0
        } else {
            slice.iter().map(Candle::range).sum::<f64>() / slice.len() as f64
        }
    };

    if atr.is_finite() && atr > MIN_ATR {
        atr
    } else {
        MIN_ATR
    }
}

fn timeframe_bias(context: Option<&crate::market_context::TimeframeContext>) -> String {
    context
        .map(|context| {
            let bias = context.bias.trim().to_uppercase();
            if bias.is_empty() {
                "UNKNOWN".to_string()
            } else {
                bias
            }
        })
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

fn macro_view(macro_flow: Option<&MacroFlowSnapshot>) -> (String, u8) {
    match macro_flow {
        Some(snapshot) => {
            let bias = snapshot.scorecard.macro_bias.trim().to_uppercase();
            let bias = if bias.is_empty() {
                "UNKNOWN".to_string()
            } else {
                bias
            };
            (
                bias,
                snapshot.scorecard.confidence.min(MAX_CONFIDENCE as u8),
            )
        }
        None => ("UNKNOWN".to_string(), 0),
    }
}

fn build_warnings(
    context: Option<&MarketContext>,
    macro_flow: Option<&MacroFlowSnapshot>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if context.is_none() {
        warnings.push("Chưa có HTF context".to_string());
    }
    if macro_flow.is_none() {
        warnings.push("Macro/Flow chưa sẵn sàng".to_string());
    }
    if context.is_some()
        && context
            .and_then(|context| context.rsi_h1.as_ref())
            .and_then(|snapshot| snapshot.value)
            .is_none()
    {
        warnings
            .push("Thiếu RSI H1 — confidence cap và anti-chase có thể không đầy đủ.".to_string());
    }
    warnings
}

fn generated_at(now_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(now_ms)
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339())
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macro_flow::MacroFlowScorecard;
    use crate::models::Timeframe;
    use crate::rsi::RsiSnapshot;

    const BASE_TS: i64 = 1_700_000_000_000;
    const STEP_MS: i64 = 15 * 60 * 1000;

    fn candle(timestamp: i64, open: f64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp,
            open,
            high,
            low,
            close,
            volume: 1000.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    /// Zigzag series with stable swing highs/lows so the dynamic S/R detector
    /// returns a support below and a resistance above the current price.
    fn zigzag_candles(count: usize) -> Vec<Candle> {
        (0..count)
            .map(|index| {
                let level = match index % 4 {
                    0 => 2670.0,
                    2 => 2690.0,
                    _ => 2680.0,
                };
                candle(
                    BASE_TS + index as i64 * STEP_MS,
                    level,
                    level + 0.5,
                    level - 0.5,
                    level,
                )
            })
            .collect()
    }

    /// Flat series: no pivots and no swing high/low beyond the current price.
    fn flat_candles(count: usize) -> Vec<Candle> {
        (0..count)
            .map(|index| {
                candle(
                    BASE_TS + index as i64 * STEP_MS,
                    2680.0,
                    2680.0,
                    2680.0,
                    2680.0,
                )
            })
            .collect()
    }

    fn config() -> BotConfig {
        BotConfig {
            lookback_period: 30,
            min_risk_reward: 2.0,
            bearish_rejection_enabled: true,
            ..BotConfig::default()
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
                flow_quality: "MIXED".to_string(),
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

    fn build(candles: &[Candle], macro_flow: Option<&MacroFlowSnapshot>) -> ScenarioResponse {
        build_scenarios(
            candles,
            &config(),
            None,
            None,
            macro_flow,
            "TEST",
            "TEST_VOLUME",
            BASE_TS,
        )
    }

    fn build_with_context(
        candles: &[Candle],
        context: Option<&MarketContext>,
        macro_flow: Option<&MacroFlowSnapshot>,
    ) -> ScenarioResponse {
        build_scenarios(
            candles,
            &config(),
            None,
            context,
            macro_flow,
            "TEST",
            "TEST_VOLUME",
            BASE_TS,
        )
    }

    fn anti_chase_context(rsi_h1: f64, rsi_h4: f64, prev_close: f64) -> MarketContext {
        let mut context = crate::market_context::build_market_context(&[], &[], &[], "TEST", None);
        context.d1.bias = "BEARISH".to_string();
        context.h4.bias = "BEARISH".to_string();
        context.h1.bias = "BEARISH".to_string();
        context.h4.atr14 = 20.0;
        context.d1.prev_close = prev_close;
        context.rsi_h1 = Some(RsiSnapshot {
            value: Some(rsi_h1),
            state: "Oversold".to_string(),
            divergence: RsiDivergence::None,
        });
        context.rsi_h4 = Some(RsiSnapshot {
            value: Some(rsi_h4),
            state: "Oversold".to_string(),
            divergence: RsiDivergence::None,
        });
        context
    }

    fn confidence_inputs<'a>(
        config: &'a BotConfig,
        rsi_h1: f64,
        rsi_h4: f64,
        divergence: RsiDivergence,
        daily_change_pct: f64,
    ) -> ScenarioInputs<'a> {
        ScenarioInputs {
            config,
            price: 2680.0,
            atr: 10.0,
            d1_bias: "BEARISH",
            h4_bias: "BEARISH",
            h1_bias: "BEARISH",
            macro_bias: "BEARISH",
            d1_structure: "LH_LL",
            h4_structure: "LH_LL",
            h1_structure: "LH_LL",
            h1_ema50_slope_3: -1.0,
            h4_ema50_slope_3: -1.0,
            volume_surge_ratio: 1.6,
            ema50_m15: 2685.0,
            rsi_value: Some(40.0),
            rsi_h1: Some(rsi_h1),
            rsi_h4: Some(rsi_h4),
            rsi_m15_cross: RsiCross::None,
            m15_divergence: divergence,
            daily_change_pct: Some(daily_change_pct),
            macro_flow: None,
        }
    }

    #[test]
    fn no_data_when_candles_are_below_lookback_requirement() {
        let response = build(&zigzag_candles(10), None);

        assert_eq!(response.status, "no_data");
        assert!(response.scenarios.is_empty());
        assert!(response.market.is_none());
        assert!(response
            .warnings
            .iter()
            .any(|warning| warning.contains("HTF")));
    }

    #[test]
    fn zigzag_market_builds_four_ranked_reference_scenarios() {
        let response = build(&zigzag_candles(60), None);

        assert_eq!(response.status, "ok");
        assert_eq!(response.scenarios.len(), 4);
        assert_eq!(response.scenarios[0].role, "PRIMARY");
        assert_eq!(response.scenarios[1].role, "ALTERNATE");
        assert_eq!(response.scenarios[2].role, "WATCHLIST");
        assert_eq!(response.scenarios[3].role, "WATCHLIST");

        let mut ids: Vec<&str> = response
            .scenarios
            .iter()
            .map(|scenario| scenario.id.as_str())
            .collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![
                "bearish_breakdown",
                "bearish_rejection",
                "bullish_bounce",
                "bullish_breakout"
            ]
        );

        let market = response.market.expect("market block");
        assert!(market.nearest_support.is_some());
        assert!(market.nearest_resistance.is_some());
        assert!(market.h4_atr >= MIN_ATR);
    }

    #[test]
    fn bullish_breakout_levels_are_ordered_and_valid() {
        let response = build(&zigzag_candles(60), None);
        let breakout = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bullish_breakout")
            .expect("bullish breakout scenario");

        assert!(breakout.valid);
        let entry = breakout.entry.expect("entry");
        let stop_loss = breakout.stop_loss.expect("stop loss");
        let take_profit_1 = breakout.take_profit_1.expect("tp1");
        let take_profit_2 = breakout.take_profit_2.expect("tp2");

        assert!(stop_loss < entry);
        assert!(entry < take_profit_1);
        assert!(take_profit_1 < take_profit_2);
        assert!(breakout.risk_reward.expect("risk reward") >= 2.0);
        assert!(breakout.notes.iter().any(|note| note.contains("breakout")));
    }

    #[test]
    fn scenario_tp1_respects_configured_min_risk_reward() {
        let config = BotConfig {
            lookback_period: 30,
            min_risk_reward: 3.0,
            ..BotConfig::default()
        };
        let response = build_scenarios(
            &zigzag_candles(60),
            &config,
            None,
            None,
            None,
            "TEST",
            "TEST_VOLUME",
            BASE_TS,
        );

        let with_levels: Vec<_> = response
            .scenarios
            .iter()
            .filter(|scenario| scenario.entry.is_some())
            .collect();
        assert!(!with_levels.is_empty());

        for scenario in with_levels {
            let entry = scenario.entry.expect("entry");
            let stop_loss = scenario.stop_loss.expect("stop loss");
            let take_profit_1 = scenario.take_profit_1.expect("tp1");
            let take_profit_2 = scenario.take_profit_2.expect("tp2");
            let risk = (entry - stop_loss).abs();
            let reward = (take_profit_1 - entry).abs();

            assert!(
                scenario.valid,
                "{} should be valid: {:?}",
                scenario.id, scenario.notes
            );
            assert!(
                reward / risk >= 3.0,
                "{} TP1 R:R {:.2} is below 3.0",
                scenario.id,
                reward / risk
            );
            assert!(
                (take_profit_2 - entry).abs() > reward,
                "{} TP2 must sit beyond TP1",
                scenario.id
            );
        }
    }

    #[test]
    fn breakdown_sl_is_anchored_above_recent_swing_high() {
        let candles = zigzag_candles(60);
        let swing_high = recent_swing_high(&candles, STRUCTURE_LOOKBACK).expect("swing high");
        let uncapped = BotConfig {
            structural_sl_atr_cap: 0.0,
            ..config()
        };
        let response = build_scenarios(
            &candles,
            &uncapped,
            None,
            None,
            None,
            "TEST",
            "TEST_VOLUME",
            BASE_TS,
        );
        let breakdown = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bearish_breakdown")
            .expect("bearish breakdown");

        let stop_loss = breakdown.stop_loss.expect("stop loss");
        assert!(
            stop_loss >= swing_high,
            "uncapped breakdown SL {stop_loss} must sit at/above swing high {swing_high}"
        );
    }

    #[test]
    fn breakdown_with_wide_structural_sl_is_capped() {
        let draft = bearish_breakdown_draft(
            2700.0,
            10.0,
            2.5,
            5.0,
            Some(2680.0),
            &[2670.0, 2660.0],
            2.0,
            Some(2800.0),
            1.0,
        );
        let levels = draft.levels.expect("levels");
        // entry 2677.5, cap 2690.0, raw structural stop 2803.0.
        assert!(levels.stop_loss <= 2690.0);
        assert!(levels.stop_loss >= 2685.0);
        assert!(validate_signal_levels(
            SignalType::Sell,
            levels.entry,
            levels.stop_loss,
            levels.take_profit_1,
            levels.take_profit_2,
            2.0,
        )
        .is_ok());
    }

    #[test]
    fn htf_confluence_beats_a_nearer_single_level() {
        let merged = merge_level_candidates(
            vec![
                LevelCandidate {
                    price: 2690.0,
                    mask: SOURCE_M15,
                    priority: 0,
                },
                LevelCandidate {
                    price: 2700.0,
                    mask: SOURCE_H4,
                    priority: 2,
                },
                LevelCandidate {
                    price: 2700.5,
                    mask: SOURCE_H1,
                    priority: 1,
                },
            ],
            2.5,
        );
        let mut resistances = merged;
        resistances.sort_by(|left, right| left.price.total_cmp(&right.price));
        let set = build_level_set(&resistances, 2684.0, 10.0);

        assert_eq!(set.nearest.map(|level| level.price), Some(2700.0));
        assert_eq!(set.nearest.map(|level| level.htf_confluence), Some(2));
    }

    #[test]
    fn far_htf_level_does_not_override_a_near_m15_level() {
        let merged = merge_level_candidates(
            vec![
                LevelCandidate {
                    price: 2690.0,
                    mask: SOURCE_M15,
                    priority: 0,
                },
                LevelCandidate {
                    price: 2750.0,
                    mask: SOURCE_D1,
                    priority: 3,
                },
            ],
            2.5,
        );
        let mut resistances = merged;
        resistances.sort_by(|left, right| left.price.total_cmp(&right.price));
        let set = build_level_set(&resistances, 2684.0, 10.0);

        assert_eq!(set.nearest.map(|level| level.price), Some(2690.0));
        assert_eq!(set.nearest.map(|level| level.htf_confluence), Some(0));
    }

    #[test]
    fn htf_merge_clusters_h4_and_h1_levels() {
        let merged = merge_level_candidates(
            vec![
                LevelCandidate {
                    price: 2700.0,
                    mask: SOURCE_H4,
                    priority: 2,
                },
                LevelCandidate {
                    price: 2700.5,
                    mask: SOURCE_H1,
                    priority: 1,
                },
            ],
            2.5,
        );

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].price, 2700.0);
        assert_eq!(merged[0].htf_confluence, 2);
    }

    #[test]
    fn anti_chase_invalidates_breakdown_and_promotes_pullback() {
        let context = anti_chase_context(18.0, 20.0, 2760.0);
        let macro_flow = macro_snapshot("BEARISH", 95);
        let response = build_with_context(&zigzag_candles(60), Some(&context), Some(&macro_flow));

        let breakdown = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bearish_breakdown")
            .expect("breakdown");
        assert!(!breakdown.valid);
        assert!(breakdown.confidence <= 65);
        assert!(breakdown
            .notes
            .iter()
            .any(|note| note.contains("Anti-chase")));

        let primary = &response.scenarios[0];
        assert_eq!(primary.role, "PRIMARY");
        assert_eq!(primary.id, "bearish_rejection");
        assert!(primary.valid);
        assert!(primary.confidence <= 85);
    }

    #[test]
    fn anti_chase_stays_inactive_when_rsi_h1_is_healthy() {
        let context = anti_chase_context(45.0, 50.0, 2760.0);
        let macro_flow = macro_snapshot("BEARISH", 95);
        let response = build_with_context(&zigzag_candles(60), Some(&context), Some(&macro_flow));

        let breakdown = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bearish_breakdown")
            .expect("breakdown");
        assert!(breakdown.valid);
        assert!(breakdown.confidence >= 90);
    }

    #[test]
    fn confidence_cap_drops_to_65_when_rsi_extreme_and_divergence_oppose() {
        let config = config();
        let inputs = confidence_inputs(&config, 18.0, 50.0, RsiDivergence::Bullish, -2.8);
        let confidence = scenario_confidence(
            "bearish_breakdown",
            "BEARISH",
            SignalType::Sell,
            Some(2680.0),
            Some(2.0),
            &inputs,
            "ALIGNED",
        );
        assert!(confidence <= 65);
    }

    #[test]
    fn confidence_cap_drops_to_75_for_opposite_divergence_alone() {
        let config = config();
        let inputs = confidence_inputs(&config, 45.0, 50.0, RsiDivergence::Bullish, 0.0);
        let confidence = scenario_confidence(
            "bearish_breakdown",
            "BEARISH",
            SignalType::Sell,
            Some(2680.0),
            Some(2.0),
            &inputs,
            "ALIGNED",
        );
        assert!(confidence <= 75);
    }

    #[test]
    fn quantum_v2_full_confluence_reaches_90() {
        let mut config = config();
        config.quantum_weighted_score_enabled = true;
        let inputs = confidence_inputs(&config, 45.0, 50.0, RsiDivergence::None, 0.0);
        let confidence = scenario_confidence(
            "bearish_breakdown",
            "BEARISH",
            SignalType::Sell,
            Some(2680.0),
            Some(2.0),
            &inputs,
            "ALIGNED",
        );
        assert!(confidence >= 90, "confidence={confidence}");
    }

    #[test]
    fn quantum_v2_counter_macro_is_capped_below_90() {
        let mut config = config();
        config.quantum_weighted_score_enabled = true;
        let mut inputs = confidence_inputs(&config, 45.0, 50.0, RsiDivergence::None, 0.0);
        inputs.macro_bias = "BULLISH";
        let confidence = scenario_confidence(
            "bearish_breakdown",
            "BEARISH",
            SignalType::Sell,
            Some(2680.0),
            Some(2.0),
            &inputs,
            "COUNTER",
        );
        assert!(confidence < 90, "confidence={confidence}");
    }

    #[test]
    fn quantum_v2_far_from_ema50_is_capped_at_75() {
        let mut config = config();
        config.quantum_weighted_score_enabled = true;
        let mut inputs = confidence_inputs(&config, 45.0, 50.0, RsiDivergence::None, 0.0);
        inputs.ema50_m15 = 2600.0;
        let confidence = scenario_confidence(
            "bearish_breakdown",
            "BEARISH",
            SignalType::Sell,
            Some(2680.0),
            Some(2.0),
            &inputs,
            "ALIGNED",
        );
        assert!(confidence <= 75, "confidence={confidence}");
    }

    #[test]
    fn rejection_at_overbought_rsi_gets_confirmation_bonus() {
        let config = config();
        let mut inputs = confidence_inputs(&config, 46.0, 24.0, RsiDivergence::Bullish, -1.8);
        inputs.rsi_value = Some(71.5);

        let confidence = scenario_confidence(
            "bearish_rejection",
            "BEARISH",
            SignalType::Sell,
            Some(2680.0),
            Some(2.0),
            &inputs,
            "ALIGNED",
        );

        assert!(confidence >= 90);
    }

    #[test]
    fn rejection_without_overbought_rsi_keeps_the_cap() {
        let config = config();
        let mut inputs = confidence_inputs(&config, 46.0, 24.0, RsiDivergence::Bullish, -1.8);
        inputs.rsi_value = Some(60.0);

        let confidence = scenario_confidence(
            "bearish_rejection",
            "BEARISH",
            SignalType::Sell,
            Some(2680.0),
            Some(2.0),
            &inputs,
            "ALIGNED",
        );

        assert!(confidence <= 65);
    }

    #[test]
    fn anti_chase_blocks_breakout_gate() {
        let config = config();
        let blocked = anti_chase_blocks_breakout(
            SignalType::Sell,
            EntryType::Breakout,
            Some(-2.8),
            Some(18.0),
            RsiDivergence::Bullish,
            &config,
        );
        assert!(blocked.is_some());

        let limit_entry = anti_chase_blocks_breakout(
            SignalType::Sell,
            EntryType::Limit,
            Some(-2.8),
            Some(18.0),
            RsiDivergence::Bullish,
            &config,
        );
        assert!(limit_entry.is_none());

        let healthy = anti_chase_blocks_breakout(
            SignalType::Sell,
            EntryType::Breakout,
            Some(-2.8),
            Some(45.0),
            RsiDivergence::None,
            &config,
        );
        assert!(healthy.is_none());
    }

    #[test]
    fn bounce_and_rejection_are_invalid_without_levels() {
        let response = build(&flat_candles(60), None);
        let bounce = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bullish_bounce")
            .expect("bullish bounce scenario");
        let rejection = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bearish_rejection")
            .expect("bearish rejection scenario");

        assert!(!bounce.valid);
        assert!(bounce.entry.is_none());
        assert!(bounce
            .notes
            .iter()
            .any(|note| note.contains("Không có vùng hỗ trợ gần")));

        assert!(!rejection.valid);
        assert!(rejection.entry.is_none());
        assert!(rejection
            .notes
            .iter()
            .any(|note| note.contains("Không có vùng kháng cự gần")));
    }

    #[test]
    fn counter_macro_lowers_confidence() {
        let candles = zigzag_candles(60);
        let aligned_macro = macro_snapshot("BULLISH", 80);
        let counter_macro = macro_snapshot("BEARISH", 80);

        let aligned = build(&candles, Some(&aligned_macro));
        let counter = build(&candles, Some(&counter_macro));
        let aligned_breakout = aligned
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bullish_breakout")
            .expect("aligned breakout");
        let counter_breakout = counter
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bullish_breakout")
            .expect("counter breakout");

        assert_eq!(aligned_breakout.macro_alignment, "ALIGNED");
        assert_eq!(counter_breakout.macro_alignment, "COUNTER");
        assert!(counter_breakout.confidence < aligned_breakout.confidence);
        assert!(counter_breakout
            .notes
            .iter()
            .any(|note| note.contains("counter-macro")));
    }

    #[test]
    fn bearish_macro_and_h4_rank_bearish_breakdown_primary() {
        let candles = zigzag_candles(60);
        let macro_flow = macro_snapshot("BEARISH", 95);
        let mut context = crate::market_context::build_market_context(&[], &[], &[], "TEST", None);
        context.d1.bias = "BEARISH".to_string();
        context.h4.bias = "BEARISH".to_string();
        context.h1.bias = "BEARISH".to_string();

        let response = build_with_context(&candles, Some(&context), Some(&macro_flow));

        assert_eq!(response.scenarios.len(), 4);
        assert_eq!(response.scenarios[0].id, "bearish_breakdown");
        assert_eq!(response.scenarios[0].role, "PRIMARY");
        assert_eq!(response.scenarios[0].bias, "BEARISH");
        assert!(response.scenarios[0].valid);
        assert_eq!(response.scenarios[0].macro_alignment, "ALIGNED");

        assert_eq!(response.scenarios[1].role, "ALTERNATE");
        assert_eq!(response.scenarios[1].bias, "BULLISH");
        assert!(response.scenarios[2..]
            .iter()
            .all(|scenario| scenario.role == "WATCHLIST"));
        assert!(response
            .scenarios
            .iter()
            .all(|scenario| scenario.priority <= response.scenarios[0].priority));
    }

    #[test]
    fn all_invalid_scenarios_still_get_primary_and_alternate() {
        // An invalid R:R configuration makes every generated scenario invalid;
        // ranking must still assign PRIMARY/ALTERNATE/WATCHLIST roles.
        let config = BotConfig {
            lookback_period: 30,
            min_risk_reward: 0.0,
            ..BotConfig::default()
        };
        let response = build_scenarios(
            &flat_candles(60),
            &config,
            None,
            None,
            None,
            "TEST",
            "TEST_VOLUME",
            BASE_TS,
        );

        assert_eq!(response.scenarios.len(), 4);
        assert!(response.scenarios.iter().all(|scenario| !scenario.valid));
        assert_eq!(response.scenarios[0].role, "PRIMARY");
        assert_eq!(response.scenarios[1].role, "ALTERNATE");
        assert_eq!(response.scenarios[2].role, "WATCHLIST");
        assert_eq!(response.scenarios[3].role, "WATCHLIST");
        assert_ne!(response.scenarios[0].bias, response.scenarios[1].bias);
    }

    #[test]
    fn priority_and_role_ordering_is_deterministic() {
        let candles = zigzag_candles(60);
        let macro_flow = macro_snapshot("BEARISH", 95);
        let mut context = crate::market_context::build_market_context(&[], &[], &[], "TEST", None);
        context.h4.bias = "BEARISH".to_string();

        let first = build_with_context(&candles, Some(&context), Some(&macro_flow));
        let second = build_with_context(&candles, Some(&context), Some(&macro_flow));

        let signature = |response: &ScenarioResponse| {
            response
                .scenarios
                .iter()
                .map(|scenario| {
                    (
                        scenario.id.clone(),
                        scenario.role.clone(),
                        scenario.priority,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(signature(&first), signature(&second));
        assert_eq!(first.scenarios[0].role, "PRIMARY");
        assert_eq!(first.scenarios[1].role, "ALTERNATE");
    }

    fn ranked_scenario(
        id: &str,
        bias: &str,
        alignment: &str,
        valid: bool,
        priority: i32,
    ) -> Scenario {
        let mut scenario = trigger_scenario("ABOVE", Some(2700.0));
        scenario.id = id.to_string();
        scenario.name = id.to_string();
        scenario.bias = bias.to_string();
        scenario.macro_alignment = alignment.to_string();
        scenario.valid = valid;
        scenario.priority = priority;
        scenario
    }

    #[test]
    fn counter_primary_is_replaced_by_aligned_scenario() {
        let ranked = rank_scenarios(vec![
            ranked_scenario("counter_valid", "BEARISH", "COUNTER", true, 300),
            ranked_scenario("aligned_invalid", "BULLISH", "ALIGNED", false, 100),
        ]);

        assert_eq!(ranked[0].id, "aligned_invalid");
        assert_eq!(ranked[0].role, "PRIMARY");
        assert!(!ranked[0].valid);
    }

    #[test]
    fn counter_only_primary_is_marked_non_tradable() {
        let ranked = rank_scenarios(vec![
            ranked_scenario("counter_a", "BEARISH", "COUNTER", true, 300),
            ranked_scenario("counter_b", "BULLISH", "COUNTER", true, 200),
        ]);

        assert_eq!(ranked[0].id, "counter_a");
        assert_eq!(ranked[0].role, "PRIMARY");
        assert!(!ranked[0].valid);
        assert!(ranked[0]
            .notes
            .iter()
            .any(|note| note.contains("COUNTER macro")));
    }

    #[test]
    fn aligned_primary_stays_primary() {
        let ranked = rank_scenarios(vec![
            ranked_scenario("aligned_valid", "BEARISH", "ALIGNED", true, 300),
            ranked_scenario("counter_valid", "BULLISH", "COUNTER", true, 250),
        ]);

        assert_eq!(ranked[0].id, "aligned_valid");
        assert_eq!(ranked[0].role, "PRIMARY");
        assert!(ranked[0].valid);
    }

    #[test]
    fn bearish_rejection_is_disabled_by_default() {
        let context = anti_chase_context(45.0, 50.0, 2680.0);
        let macro_flow = macro_snapshot("BEARISH", 95);
        let mut config = config();
        config.bearish_rejection_enabled = false;

        let response = build_scenarios(
            &zigzag_candles(60),
            &config,
            None,
            Some(&context),
            Some(&macro_flow),
            "TEST",
            "TEST_VOLUME",
            BASE_TS,
        );
        let rejection = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bearish_rejection")
            .expect("bearish rejection");
        assert!(!rejection.valid);
        assert!(rejection
            .notes
            .iter()
            .any(|note| note.contains("bearish_rejection_enabled")));

        config.bearish_rejection_enabled = true;
        let enabled = build_scenarios(
            &zigzag_candles(60),
            &config,
            None,
            Some(&context),
            Some(&macro_flow),
            "TEST",
            "TEST_VOLUME",
            BASE_TS,
        );
        let rejection_enabled = enabled
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bearish_rejection")
            .expect("enabled bearish rejection");
        assert!(rejection_enabled.valid);
    }

    #[test]
    fn build_scenarios_is_pure_and_repeatable() {
        let candles = zigzag_candles(60);
        let first = build(&candles, None);
        let second = build(&candles, None);

        assert_eq!(first.status, second.status);
        assert_eq!(first.scenarios.len(), second.scenarios.len());
        for (left, right) in first.scenarios.iter().zip(second.scenarios.iter()) {
            assert_eq!(left.id, right.id);
            assert_eq!(left.entry, right.entry);
            assert_eq!(left.stop_loss, right.stop_loss);
            assert_eq!(left.take_profit_1, right.take_profit_1);
            assert_eq!(left.take_profit_2, right.take_profit_2);
            assert_eq!(left.valid, right.valid);
            assert_eq!(left.role, right.role);
            assert_eq!(left.priority, right.priority);
        }
    }

    #[test]
    fn response_serializes_with_the_documented_api_shape() {
        let response = build(&zigzag_candles(60), None);
        let value = serde_json::to_value(&response).expect("serialize scenario response");

        for key in [
            "status",
            "generated_at",
            "price",
            "source",
            "volume_source",
            "market",
            "scenarios",
            "warnings",
        ] {
            assert!(value.get(key).is_some(), "missing response key: {key}");
        }

        let market = &value["market"];
        for key in [
            "h4_bias",
            "h1_bias",
            "d1_bias",
            "h4_atr",
            "nearest_support",
            "nearest_resistance",
            "rsi_m15",
            "rsi_state",
            "macro_bias",
            "macro_confidence",
        ] {
            assert!(market.get(key).is_some(), "missing market key: {key}");
        }

        let scenario = &value["scenarios"][0];
        for key in [
            "id",
            "name",
            "bias",
            "trigger",
            "condition",
            "entry",
            "stop_loss",
            "take_profit_1",
            "take_profit_2",
            "risk_reward",
            "trigger_price",
            "trigger_direction",
            "trigger_timeframe",
            "invalidation",
            "confidence",
            "macro_alignment",
            "valid",
            "role",
            "priority",
            "notes",
        ] {
            assert!(scenario.get(key).is_some(), "missing scenario key: {key}");
        }
    }

    fn trigger_scenario(direction: &str, trigger_price: Option<f64>) -> Scenario {
        Scenario {
            id: "test_trigger".to_string(),
            name: "Test trigger".to_string(),
            bias: "BULLISH".to_string(),
            trigger: "Test".to_string(),
            condition: "Test".to_string(),
            entry: Some(2701.0),
            stop_loss: Some(2691.0),
            take_profit_1: Some(2721.0),
            take_profit_2: Some(2731.0),
            risk_reward: Some(2.0),
            trigger_price,
            trigger_direction: direction.to_string(),
            trigger_timeframe: "M15".to_string(),
            invalidation: "Test".to_string(),
            confidence: 80,
            macro_alignment: "ALIGNED".to_string(),
            valid: true,
            role: "PRIMARY".to_string(),
            priority: 300,
            notes: Vec::new(),
        }
    }

    #[test]
    fn scenario_triggered_above_uses_close_and_strict_boundary() {
        let scenario = trigger_scenario("ABOVE", Some(2700.0));

        assert!(scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2690.0, 2705.0, 2689.0, 2700.01)
        ));
        assert!(!scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2690.0, 2705.0, 2689.0, 2700.0)
        ));
        assert!(!scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2690.0, 2705.0, 2689.0, 2699.99)
        ));
    }

    #[test]
    fn scenario_triggered_below_uses_close_and_strict_boundary() {
        let scenario = trigger_scenario("BELOW", Some(2668.0));

        assert!(scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2670.0, 2671.0, 2660.0, 2667.99)
        ));
        assert!(!scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2670.0, 2671.0, 2660.0, 2668.0)
        ));
        assert!(!scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2670.0, 2671.0, 2660.0, 2668.01)
        ));
    }

    #[test]
    fn scenario_triggered_touch_uses_wick_range_and_includes_boundaries() {
        let scenario = trigger_scenario("TOUCH", Some(2670.0));

        assert!(scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2672.0, 2678.0, 2668.0, 2675.0)
        ));
        assert!(scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2672.0, 2678.0, 2670.0, 2675.0)
        ));
        assert!(scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2665.0, 2670.0, 2662.0, 2668.0)
        ));
        assert!(!scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2672.0, 2678.0, 2670.01, 2675.0)
        ));
        assert!(!scenario_triggered(
            &scenario,
            &candle(BASE_TS, 2665.0, 2669.99, 2662.0, 2668.0)
        ));
    }

    #[test]
    fn scenario_triggered_none_or_missing_price_never_fires() {
        let none = trigger_scenario("NONE", Some(2700.0));
        let above_without_price = trigger_scenario("ABOVE", None);
        let unknown = trigger_scenario("SIDEWAYS", Some(2700.0));
        let boundary_candle = candle(BASE_TS, 2700.0, 2700.0, 2700.0, 2700.0);

        assert!(!scenario_triggered(&none, &boundary_candle));
        assert!(!scenario_triggered(&above_without_price, &boundary_candle));
        assert!(!scenario_triggered(&unknown, &boundary_candle));
    }

    #[test]
    fn built_scenarios_expose_structured_breakout_triggers() {
        let response = build(&zigzag_candles(60), None);
        let breakout = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bullish_breakout")
            .expect("bullish breakout");
        let breakdown = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bearish_breakdown")
            .expect("bearish breakdown");

        assert_eq!(breakout.trigger_direction, "ABOVE");
        assert_eq!(breakout.trigger_timeframe, "M15");
        assert_eq!(breakout.trigger_price, breakout.entry);

        assert_eq!(breakdown.trigger_direction, "BELOW");
        assert_eq!(breakdown.trigger_timeframe, "M15");
        assert_eq!(breakdown.trigger_price, breakdown.entry);
    }

    #[test]
    fn built_bounce_and_rejection_use_touch_at_the_level() {
        let mut context = crate::market_context::build_market_context(&[], &[], &[], "TEST", None);
        context.h4.atr14 = 20.0;
        let response = build_with_context(&zigzag_candles(60), Some(&context), None);
        let market = response.market.as_ref().expect("market block");
        let bounce = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bullish_bounce")
            .expect("bullish bounce");
        let rejection = response
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "bearish_rejection")
            .expect("bearish rejection");

        assert_eq!(bounce.trigger_direction, "TOUCH");
        assert_eq!(bounce.trigger_timeframe, "M15");
        assert_eq!(bounce.trigger_price, market.nearest_support);

        assert_eq!(rejection.trigger_direction, "TOUCH");
        assert_eq!(rejection.trigger_timeframe, "M15");
        assert_eq!(rejection.trigger_price, market.nearest_resistance);
    }

    #[test]
    fn scenarios_without_levels_have_no_trigger() {
        let response = build(&flat_candles(60), None);

        assert!(response
            .scenarios
            .iter()
            .all(|scenario| scenario.trigger_price.is_none()));
        assert!(response
            .scenarios
            .iter()
            .all(|scenario| scenario.trigger_direction == "NONE"));
    }
}
