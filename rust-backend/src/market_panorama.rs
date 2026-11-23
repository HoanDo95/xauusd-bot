//! Deterministic market panorama for the manual `/start-entry` view.
//!
//! Read-only: it never creates a signal, consumes quota, calls AI or places an
//! order. AI is attached by the caller; this module only assembles the
//! deterministic volatility/structure/zones/scenarios/news snapshot.

use crate::analyzer::GoldMarketAnalyzer;
use crate::api::{compute_market_outlook, current_scenarios, AppState};
use crate::market_context::{MarketContext, TimeframeContext};
use crate::market_view::{H1Confirmation, MarketScore, OutlookScenario};
use crate::models::SupportResistanceZone;
use crate::news_filter::ImpactLevel;
use crate::scenarios::Scenario;
use crate::storage::PersistentStore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketPanorama {
    pub generated_at: String,
    pub price: Option<f64>,
    pub session: String,
    pub weekend: bool,
    pub source: String,
    pub volume_source: String,
    pub volatility: VolatilityBlock,
    pub structure: StructureBlock,
    pub zones: ZonesBlock,
    pub scenarios: Vec<PanoramaScenario>,
    pub macro_block: Option<MacroBlock>,
    pub news: NewsBlock,
    pub score: Option<MarketScore>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VolatilityBlock {
    pub h4_atr: Option<f64>,
    pub atr_pct: Option<f64>,
    pub daily_change_pct: Option<f64>,
    pub spread_usd: Option<f64>,
    pub spread_proxy_atr: Option<f64>,
    pub regime: String,
    pub m15_range: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructureBlock {
    pub d1_bias: String,
    pub h4_bias: String,
    pub h1_bias: String,
    pub d1_structure: String,
    pub h4_structure: String,
    pub h1_structure: String,
    /// Advisory structure state from the last closed candles:
    /// CONTINUATION | PULLBACK | POTENTIAL_BULLISH_SHIFT | POTENTIAL_BEARISH_SHIFT | RANGE.
    #[serde(default)]
    pub state: String,
    pub h4_atr: Option<f64>,
    pub price_vs_ema50_m15: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZonesBlock {
    pub nearest_support: Option<f64>,
    pub nearest_resistance: Option<f64>,
    pub supports: Vec<ZoneView>,
    pub resistances: Vec<ZoneView>,
    pub htf_levels: Vec<HtfLevelView>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneView {
    pub price: f64,
    pub min_price: f64,
    pub max_price: f64,
    pub touch_count: usize,
    pub strength: f64,
    pub distance_usd: f64,
    pub distance_atr: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HtfLevelView {
    pub timeframe: String,
    pub kind: String,
    pub price: f64,
    pub distance_atr: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PanoramaScenario {
    pub role: String,
    pub scenario_id: String,
    pub name: String,
    pub bias: String,
    pub action: String,
    pub valid: bool,
    pub zone_low: f64,
    pub zone_high: f64,
    pub trigger_price: Option<f64>,
    pub trigger_direction: String,
    pub trigger_timeframe: String,
    pub entry: Option<f64>,
    pub stop_loss: Option<f64>,
    pub take_profit_1: Option<f64>,
    pub take_profit_2: Option<f64>,
    pub risk_reward: Option<f64>,
    pub confidence: u8,
    pub macro_alignment: String,
    pub invalidation: String,
    pub h1_confirmation: Option<H1Confirmation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MacroBlock {
    pub bias: String,
    pub confidence: u8,
    pub score: i32,
    pub flow_quality: String,
    pub alignment: String,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewsBlock {
    pub next_high_impact_title: Option<String>,
    pub minutes_until: Option<i64>,
    pub kill_switch_active: bool,
    pub kill_switch_reason: String,
}

fn volatility_regime(atr_pct: Option<f64>) -> String {
    match atr_pct {
        Some(value) if value < 0.5 => "LOW".to_string(),
        Some(value) if value < 1.0 => "NORMAL".to_string(),
        Some(_) => "HIGH".to_string(),
        None => "UNKNOWN".to_string(),
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Display-zone cluster tolerance. Signal detection keeps its own 2.5 fixed
/// tolerance so entry rules do not change; only the panorama/watch zones scale
/// with H4 volatility.
fn zone_tolerance(h4_atr: Option<f64>) -> f64 {
    (h4_atr.unwrap_or(10.0) * 0.10).clamp(2.5, 8.0)
}

fn zone_view(zone: &SupportResistanceZone, price: f64, h4_atr: Option<f64>) -> ZoneView {
    let distance = (price - zone.price).abs();
    ZoneView {
        price: round2(zone.price),
        min_price: round2(zone.min_price),
        max_price: round2(zone.max_price),
        touch_count: zone.touch_count,
        strength: (zone.strength_score * 100.0).round() / 100.0,
        distance_usd: round2(distance),
        distance_atr: h4_atr
            .filter(|atr| atr.is_finite() && *atr > 0.0)
            .map(|atr| (distance / atr * 100.0).round() / 100.0),
    }
}

fn top_zones(
    zones: &[SupportResistanceZone],
    price: f64,
    h4_atr: Option<f64>,
    limit: usize,
) -> Vec<ZoneView> {
    let mut views: Vec<ZoneView> = zones
        .iter()
        .map(|zone| zone_view(zone, price, h4_atr))
        .collect();
    views.sort_by(|left, right| left.distance_usd.total_cmp(&right.distance_usd));
    views.truncate(limit);
    views
}

fn htf_levels(
    context: Option<&MarketContext>,
    price: f64,
    h4_atr: Option<f64>,
) -> Vec<HtfLevelView> {
    let mut levels: Vec<HtfLevelView> = Vec::new();
    let Some(context) = context else {
        return levels;
    };
    for timeframe in [&context.d1, &context.h4, &context.h1] {
        push_htf_level(&mut levels, timeframe, price, h4_atr);
    }
    levels.sort_by(|left, right| {
        let left_distance = (left.price - price).abs();
        let right_distance = (right.price - price).abs();
        left_distance.total_cmp(&right_distance)
    });
    levels.dedup_by(|left, right| (left.price - right.price).abs() < 0.01);
    levels.truncate(6);
    levels
}

fn push_htf_level(
    levels: &mut Vec<HtfLevelView>,
    timeframe: &TimeframeContext,
    price: f64,
    h4_atr: Option<f64>,
) {
    for (kind, level) in [
        ("SUPPORT", timeframe.nearest_support),
        ("RESISTANCE", timeframe.nearest_resistance),
    ] {
        let Some(level) = level.filter(|value| value.is_finite() && *value > 0.0) else {
            continue;
        };
        let distance = (price - level).abs();
        levels.push(HtfLevelView {
            timeframe: timeframe.timeframe.clone(),
            kind: kind.to_string(),
            price: round2(level),
            distance_atr: h4_atr
                .filter(|atr| atr.is_finite() && *atr > 0.0)
                .map(|atr| (distance / atr * 100.0).round() / 100.0),
        });
    }
}

fn action_for_bias(bias: &str) -> String {
    match bias.trim().to_uppercase().as_str() {
        "BULLISH" => "WATCH_BUY",
        "BEARISH" => "WATCH_SELL",
        _ => "STAND_ASIDE",
    }
    .to_string()
}

fn panorama_scenario(
    scenario: &Scenario,
    outlook_scenario: Option<&OutlookScenario>,
    price: f64,
    h4_atr: Option<f64>,
) -> PanoramaScenario {
    let anchor = scenario.entry.or(scenario.trigger_price).unwrap_or(price);
    let buffer = (h4_atr.unwrap_or(10.0) * 0.10).max(1.5);
    let (zone_low, zone_high, action, h1_confirmation) = match outlook_scenario {
        Some(outlook) => (
            outlook.zone_low,
            outlook.zone_high,
            outlook.action.clone(),
            outlook.h1_confirmation.clone(),
        ),
        None => (
            round2(anchor - buffer),
            round2(anchor + buffer),
            action_for_bias(&scenario.bias),
            None,
        ),
    };
    PanoramaScenario {
        role: scenario.role.trim().to_uppercase(),
        scenario_id: scenario.id.clone(),
        name: scenario.name.clone(),
        bias: scenario.bias.clone(),
        action,
        valid: scenario.valid,
        zone_low,
        zone_high,
        trigger_price: scenario.trigger_price.map(round2),
        trigger_direction: scenario.trigger_direction.clone(),
        trigger_timeframe: scenario.trigger_timeframe.clone(),
        entry: scenario.entry.map(round2),
        stop_loss: scenario.stop_loss.map(round2),
        take_profit_1: scenario.take_profit_1.map(round2),
        take_profit_2: scenario.take_profit_2.map(round2),
        risk_reward: scenario
            .risk_reward
            .map(|value| (value * 100.0).round() / 100.0),
        confidence: scenario.confidence,
        macro_alignment: scenario.macro_alignment.clone(),
        invalidation: scenario.invalidation.clone(),
        h1_confirmation,
    }
}

fn next_high_impact_news(
    events: &[crate::news_filter::EconomicEvent],
    now: DateTime<Utc>,
) -> Option<&crate::news_filter::EconomicEvent> {
    events
        .iter()
        .filter(|event| {
            event.currency.eq_ignore_ascii_case("USD")
                && event.impact == ImpactLevel::High
                && event.scheduled_time > now
        })
        .min_by_key(|event| event.scheduled_time)
}

/// Build the deterministic panorama. AI is intentionally absent here.
pub async fn build_market_panorama(state: &AppState) -> MarketPanorama {
    let config = state.config.read().await.clone();
    let candles = state.candles.read().await.clone();
    let live_price = *state.live_price.read().await;
    let context = state.market_context.read().await.clone();
    let macro_flow = state.macro_flow.read().await.clone();
    let scenarios = current_scenarios(state).await;
    let outlook = compute_market_outlook(state).await;
    let source = state.active_data_source.read().await.clone();
    let volume_source = state.active_volume_source.read().await.clone();
    let spread_usd = *state.live_spread.read().await;
    let now = Utc::now();

    let price = outlook
        .price
        .or(scenarios.price)
        .or(live_price)
        .filter(|value| value.is_finite() && *value > 0.0);
    let h4_atr = context
        .as_ref()
        .map(|context| context.h4.atr14)
        .or_else(|| scenarios.market.as_ref().map(|market| market.h4_atr))
        .filter(|value| value.is_finite() && *value > 0.0);
    let atr_pct = price
        .zip(h4_atr)
        .map(|(price, atr)| (atr / price * 10000.0).round() / 100.0);
    let daily_change_pct = scenarios
        .market
        .as_ref()
        .and_then(|market| market.daily_change_pct);
    let m15_range = candles
        .last()
        .map(|candle| round2(candle.high - candle.low));

    let (supports, resistances) = match price {
        Some(price) => {
            let (supports, resistances) = GoldMarketAnalyzer::detect_dynamic_sr_zones(
                &candles,
                config.lookback_period.max(5),
                zone_tolerance(h4_atr),
            );
            (
                top_zones(&supports, price, h4_atr, 3),
                top_zones(&resistances, price, h4_atr, 3),
            )
        }
        None => (Vec::new(), Vec::new()),
    };

    let htf = price
        .map(|price| htf_levels(context.as_ref(), price, h4_atr))
        .unwrap_or_default();

    let outlook_map: HashMap<&str, &OutlookScenario> = outlook
        .primary
        .iter()
        .chain(outlook.alternative.iter())
        .map(|scenario| (scenario.scenario_id.as_str(), scenario))
        .collect();
    let panorama_scenarios = price
        .map(|price| {
            scenarios
                .scenarios
                .iter()
                .map(|scenario| {
                    panorama_scenario(
                        scenario,
                        outlook_map.get(scenario.id.as_str()).copied(),
                        price,
                        h4_atr,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let primary_alignment = panorama_scenarios
        .iter()
        .find(|scenario| scenario.role.eq_ignore_ascii_case("PRIMARY"))
        .map(|scenario| scenario.macro_alignment.clone())
        .unwrap_or_else(|| "UNKNOWN".to_string());

    let macro_block = macro_flow.as_ref().map(|snapshot| MacroBlock {
        bias: snapshot.scorecard.macro_bias.clone(),
        confidence: snapshot.scorecard.confidence,
        score: snapshot.scorecard.score,
        flow_quality: snapshot.scorecard.flow_quality.clone(),
        alignment: primary_alignment,
        note: snapshot
            .scorecard
            .notes
            .first()
            .cloned()
            .unwrap_or_default(),
    });

    let news = {
        let news_filter = state.news_filter.read().await;
        let (kill_switch_active, _active_event, kill_switch_reason) =
            news_filter.evaluate_news_status(now, &config);
        let next = next_high_impact_news(&news_filter.calendar_events, now);
        NewsBlock {
            next_high_impact_title: next.map(|event| event.title.clone()),
            minutes_until: next.map(|event| (event.scheduled_time - now).num_minutes()),
            kill_switch_active,
            kill_switch_reason,
        }
    };

    let h1_structure_for_state = context
        .as_ref()
        .map(|context| context.h1.structure.clone())
        .unwrap_or_else(|| "UNKNOWN".to_string());
    let nearest_support_for_state = supports.first().map(|zone| zone.price);
    let nearest_resistance_for_state = resistances.first().map(|zone| zone.price);
    let structure_state = match price {
        Some(price) => {
            if nearest_resistance_for_state.is_some_and(|level| price > level)
                && h1_structure_for_state == "LH_LL"
            {
                "POTENTIAL_BULLISH_SHIFT"
            } else if nearest_support_for_state.is_some_and(|level| price < level)
                && h1_structure_for_state == "HH_HL"
            {
                "POTENTIAL_BEARISH_SHIFT"
            } else if h1_structure_for_state == "LH_LL" || h1_structure_for_state == "HH_HL" {
                "PULLBACK"
            } else {
                "RANGE"
            }
        }
        None => "UNKNOWN",
    }
    .to_string();

    let structure = StructureBlock {
        d1_bias: context
            .as_ref()
            .map(|context| context.d1.bias.clone())
            .unwrap_or_else(|| outlook.d1_bias.clone()),
        h4_bias: context
            .as_ref()
            .map(|context| context.h4.bias.clone())
            .unwrap_or_else(|| outlook.h4_bias.clone()),
        h1_bias: context
            .as_ref()
            .map(|context| context.h1.bias.clone())
            .unwrap_or_else(|| outlook.h1_bias.clone()),
        d1_structure: context
            .as_ref()
            .map(|context| context.d1.structure.clone())
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        h4_structure: context
            .as_ref()
            .map(|context| context.h4.structure.clone())
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        h1_structure: context
            .as_ref()
            .map(|context| context.h1.structure.clone())
            .unwrap_or_else(|| "UNKNOWN".to_string()),
        state: structure_state,
        h4_atr,
        price_vs_ema50_m15: price.map(|price| {
            let ema50 = GoldMarketAnalyzer::calculate_ema(&candles, 50);
            if ema50 > 0.0 {
                round2(price - ema50)
            } else {
                0.0
            }
        }),
    };

    MarketPanorama {
        generated_at: now.to_rfc3339(),
        price: price.map(round2),
        session: outlook.session.clone(),
        weekend: PersistentStore::is_market_closed_weekend(now),
        source,
        volume_source,
        volatility: VolatilityBlock {
            h4_atr,
            atr_pct,
            daily_change_pct,
            spread_usd,
            spread_proxy_atr: outlook.spread_proxy_atr,
            regime: volatility_regime(atr_pct),
            m15_range,
        },
        structure,
        zones: ZonesBlock {
            nearest_support: supports.first().map(|zone| zone.price),
            nearest_resistance: resistances.first().map(|zone| zone.price),
            supports,
            resistances,
            htf_levels: htf,
        },
        scenarios: panorama_scenarios,
        macro_block,
        news,
        score: outlook.score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_zone_tolerance_scales_with_h4_atr() {
        assert_eq!(zone_tolerance(None), 2.5);
        assert_eq!(zone_tolerance(Some(20.0)), 2.5);
        assert_eq!(zone_tolerance(Some(30.0)), 3.0);
        assert_eq!(zone_tolerance(Some(100.0)), 8.0);
    }
}
