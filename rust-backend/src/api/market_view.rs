//! Read-only MARKET VIEW API + shared outlook computation.
//!
//! The view is deterministic and always available (including outside the
//! 13:00–23:00 session). Hard gates produce NO_TRADE; RSI/EMA never silence the
//! view. This endpoint never sends Telegram, inserts a signal or consumes quota.

use super::{current_scenarios, AppState};
use crate::market_strategist::{review_market_strategist, StrategistView};
use crate::market_view::{build_outlook, MarketOutlook, OutlookInput};
use crate::models::{BotConfig, Candle, SignalStatus};
use crate::news_filter::ImpactLevel;
use crate::storage::PersistentStore;
use crate::trigger_journal::{record, TriggerEvent};
use axum::{extract::State, response::IntoResponse, Json};
use chrono::{DateTime, Utc};

/// Build the current Market Outlook from live state. Shared by the API and the
/// H1 Telegram worker.
pub async fn compute_market_outlook(state: &AppState) -> MarketOutlook {
    let config = state.config.read().await.clone();
    let now = Utc::now();
    let scenarios = current_scenarios(state).await;
    let context = state.market_context.read().await.clone();
    let session_in = PersistentStore::is_within_golden_session(now, config.session_filter_enabled);
    let last_candle = state.candles.read().await.last().cloned();
    let spread = match last_candle.as_ref() {
        Some(candle) => spread_proxy_atr(state, candle).await,
        None => None,
    };
    let no_trade_reason = no_trade_reason(state, &config, now, last_candle.as_ref(), spread).await;
    let news_notes = upcoming_news_notes(state, now).await;
    let active_sniper = active_sniper(state).await;
    let strategist = if config.market_strategist_enabled {
        state.market_strategist_view.read().await.clone()
    } else {
        None
    };

    build_outlook(OutlookInput {
        scenarios: &scenarios,
        context: context.as_ref(),
        now,
        session_in,
        no_trade_reason,
        news_notes,
        spread_proxy_atr: spread,
        active_sniper,
        strategist,
    })
}

pub async fn get_market_view(State(state): State<AppState>) -> impl IntoResponse {
    Json(compute_market_outlook(&state).await)
}

/// Manual, read-only AI strategist refresh for testing/A-B. No Telegram, no
/// signal, no quota. The H1 worker calls the same function automatically.
pub async fn refresh_market_strategist_view(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let outlook = compute_market_outlook(&state).await;
    Json(refresh_market_strategist(&state, &outlook, &config).await)
}

/// Call the strategist, cache the view and journal it. Shared by the H1 worker
/// and the manual refresh endpoint.
pub(crate) async fn refresh_market_strategist(
    state: &AppState,
    outlook: &MarketOutlook,
    config: &BotConfig,
) -> Option<StrategistView> {
    let scenarios = current_scenarios(state).await;
    let context = state.market_context.read().await.clone();
    let news_notes: Vec<String> = outlook
        .risk_notes
        .iter()
        .filter(|note| note.starts_with("Tin đỏ"))
        .cloned()
        .collect();
    let view = review_market_strategist(
        &scenarios,
        context.as_ref(),
        &news_notes,
        outlook.no_trade_reason.as_deref(),
        outlook.session == "IN",
    )
    .await;
    *state.market_strategist_view.write().await = view.clone();
    record_strategist_event(state, config, outlook, view.as_ref());
    view
}

fn record_strategist_event(
    state: &AppState,
    config: &BotConfig,
    outlook: &MarketOutlook,
    view: Option<&StrategistView>,
) {
    let mut event = TriggerEvent::new("strategist_view");
    event.created_at = Some(Utc::now());
    event.source = Some("market_strategist".to_string());
    event.regime = Some(outlook.structure.clone());
    event.session = Some(outlook.session.clone());
    event.ai_model = Some(
        view.map(|view| view.model.clone())
            .unwrap_or_else(|| "unavailable".to_string()),
    );
    event.prompt_version = Some(
        view.map(|view| view.prompt_version.clone())
            .unwrap_or_else(|| "strategist_v1".to_string()),
    );
    event.market_view = view.map(|view| view.market_view.clone());
    event.scenario_id = view.and_then(|view| view.primary_scenario_id.clone());
    event.ai_action = view.and_then(|view| view.primary_action.clone());
    event.reason = if view.is_none() {
        Some("strategist_unavailable".to_string())
    } else {
        None
    };
    record(
        &state.trigger_journal,
        config.trigger_journal_enabled,
        &event,
    );
}

/// Hard gates only. RSI/EMA/macro are scoring inputs, never NO_TRADE reasons.
async fn no_trade_reason(
    state: &AppState,
    config: &BotConfig,
    now: DateTime<Utc>,
    last_candle: Option<&Candle>,
    spread_proxy_atr: Option<f64>,
) -> Option<String> {
    if PersistentStore::is_market_closed_weekend(now) {
        return Some("Cuối tuần — thị trường vàng đóng cửa.".to_string());
    }

    let news_block = {
        let news = state.news_filter.read().await;
        news.get_current_blocking_event(now, config).map(|event| {
            format!(
                "Tin đỏ High Impact USD: {} — NO_TRADE trong ±{} phút.",
                event.title, config.pre_news_buffer_minutes
            )
        })
    };
    if let Some(reason) = news_block {
        return Some(reason);
    }

    let source = state.active_data_source.read().await.clone();
    if source.eq_ignore_ascii_case("FALLBACK_SEED")
        || source.eq_ignore_ascii_case("SPOT_BASIS_UNAVAILABLE")
    {
        return Some("Nguồn dữ liệu fallback/stale — NO_TRADE.".to_string());
    }

    let Some(last_candle) = last_candle else {
        return Some("Chưa có nến M15 — NO_TRADE.".to_string());
    };
    let close_ms = last_candle.timestamp.saturating_add(15 * 60 * 1000);
    let age_ms = now.timestamp_millis().saturating_sub(close_ms);
    if age_ms > 30 * 60 * 1000 {
        return Some("Dữ liệu M15 cũ hơn 30 phút — NO_TRADE.".to_string());
    }

    if let Some(spread) = spread_proxy_atr {
        if spread > config.max_spread_proxy_atr {
            return Some(format!(
                "Spread proxy {:.2}×H4 ATR vượt ngưỡng {:.2} — NO_TRADE.",
                spread, config.max_spread_proxy_atr
            ));
        }
    }

    None
}

/// Pure spread proxy formula: |live − close| / H4 ATR.
pub(crate) fn spread_proxy(live: f64, close: f64, h4_atr: f64) -> Option<f64> {
    if !live.is_finite() || !close.is_finite() || !h4_atr.is_finite() || h4_atr <= 0.0 {
        return None;
    }
    Some(((live - close).abs() / h4_atr).max(0.0))
}

/// Proxy for abnormal spread without a broker bid/ask feed:
/// |live − last M15 close| / H4 ATR. Documented as a proxy, not real spread.
pub(crate) async fn spread_proxy_atr(state: &AppState, last_candle: &Candle) -> Option<f64> {
    let live = (*state.live_price.read().await)?;
    let context = state.market_context.read().await;
    let h4_atr = context.as_ref().map(|context| context.h4.atr14)?;
    spread_proxy(live, last_candle.close, h4_atr)
}

async fn upcoming_news_notes(state: &AppState, now: DateTime<Utc>) -> Vec<String> {
    let events = state.news_filter.read().await.calendar_events.clone();
    events
        .iter()
        .filter(|event| {
            event.currency.eq_ignore_ascii_case("USD") && event.impact == ImpactLevel::High
        })
        .filter_map(|event| {
            let minutes = (event.scheduled_time - now).num_minutes();
            if !(-30..=180).contains(&minutes) {
                return None;
            }
            let timing = if minutes >= 0 {
                format!("sau {minutes} phút")
            } else {
                format!("vừa xong {} phút", -minutes)
            };
            Some(format!(
                "Tin đỏ sắp tới: {} lúc {} UTC ({}).",
                event.title,
                event.scheduled_time.format("%H:%M"),
                timing
            ))
        })
        .take(3)
        .collect()
}

async fn active_sniper(state: &AppState) -> bool {
    let signals = state.signals.read().await;
    signals
        .iter()
        .any(|signal| !signal.is_simulated && matches!(signal.status, SignalStatus::Active))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_proxy_is_relative_to_h4_atr() {
        assert_eq!(spread_proxy(2700.0, 2695.0, 10.0), Some(0.5));
        assert_eq!(spread_proxy(2695.0, 2700.0, 10.0), Some(0.5));
        assert_eq!(spread_proxy(2700.0, 2700.0, 10.0), Some(0.0));
        assert_eq!(spread_proxy(2700.0, 2695.0, 0.0), None);
        assert_eq!(spread_proxy(f64::NAN, 2695.0, 10.0), None);
    }
}
