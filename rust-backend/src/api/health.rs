use super::{price_source_label, AppState};
use crate::models::Timeframe;
use crate::storage::PersistentStore;
use axum::{extract::State, response::IntoResponse, Json};
use chrono::{DateTime, Utc};
use serde_json::json;

fn timeframe_millis(timeframe: Timeframe) -> i64 {
    match timeframe {
        Timeframe::M15 => 15 * 60 * 1000,
        Timeframe::H1 => 60 * 60 * 1000,
        Timeframe::H4 => 4 * 60 * 60 * 1000,
    }
}

/// Feed chỉ "ok" khi nguồn hiện tại không phải fallback/không có basis và
/// scanner vừa tick trong vòng 180 giây. Không có lần quét nào cũng là degraded.
fn feed_is_ok(
    active_data_source: &str,
    last_scan_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    if active_data_source.eq_ignore_ascii_case("FALLBACK_SEED")
        || active_data_source.eq_ignore_ascii_case("SPOT_BASIS_UNAVAILABLE")
    {
        return false;
    }

    last_scan_at
        .map(|at| (now - at).num_seconds() <= 180)
        .unwrap_or(false)
}

pub(super) async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let signals = state.signals.read().await.clone();
    let live_price = *state.live_price.read().await;
    let active_data_source = state.active_data_source.read().await.clone();
    let spot_price = *state.spot_price.read().await;
    let spot_basis = *state.spot_basis.read().await;
    let last_scan_at = *state.last_scan_at.read().await;
    let spot_basis_at = *state.spot_basis_at.read().await;
    let price_source = price_source_label(&active_data_source, spot_basis);
    let now = Utc::now();

    let (candles_count, candle_age_secs) = {
        let candles = state.candles.read().await;
        let age = candles.last().map(|candle| {
            let close_time = candle
                .timestamp
                .saturating_add(timeframe_millis(candle.timeframe));
            ((now.timestamp_millis() - close_time) / 1000).max(0)
        });
        (candles.len(), age)
    };

    let signals_today = PersistentStore::count_live_signals_today(&signals, now);
    let daily_quota_reached =
        PersistentStore::has_reached_daily_quota(&signals, config.max_signals_per_day, now);
    let is_market_safe = state
        .news_filter
        .read()
        .await
        .check_kill_switch(now, &config);

    let last_scan_age_secs = last_scan_at.map(|at| (now - at).num_seconds().max(0));
    let spot_basis_age_secs = spot_basis
        .and(spot_basis_at)
        .map(|at| (now - at).num_seconds().max(0));
    let feed_ok = feed_is_ok(&active_data_source, last_scan_at, now);
    let status = if feed_ok { "healthy" } else { "degraded" };

    Json(json!({
        "status": status,
        "service": "Gold Signal Bot (Rust Backend)",
        "engine": "Tokio 1.40 / Axum 0.7 High Performance",
        "asset": "XAU/USD",
        "version": "1.3.0",
        "symbol": config.symbol,
        "timeframe": config.timeframe,
        "isActive": config.is_active,
        "candlesCount": candles_count,
        "signalsCount": signals.len(),
        "signalsToday": signals_today,
        "maxSignalsPerDay": config.max_signals_per_day,
        "dailyQuotaReached": daily_quota_reached,
        "sessionFilterEnabled": config.session_filter_enabled,
        "h4FilterEnabled": config.htf_h4_filter_enabled,
        "d1FilterEnabled": config.htf_d1_filter_enabled,
        "aiReviewEnabled": config.ai_signal_review_enabled,
        "volumeDeltaFilterEnabled": config.volume_delta_filter_enabled,
        "rsiFilterEnabled": config.rsi_filter_enabled,
        "dataSource": active_data_source,
        "priceSource": price_source,
        "spotPrice": spot_price,
        "spotBasis": spot_basis,
        "livePrice": live_price,
        "feedOk": feed_ok,
        "lastScanAt": last_scan_at.map(|at| at.to_rfc3339()),
        "lastScanAgeSecs": last_scan_age_secs,
        "candleAgeSecs": candle_age_secs,
        "spotBasisAgeSecs": spot_basis_age_secs,
        "killSwitchActive": !is_market_safe,
        "cron_schedule": "07:00 AM Daily",
        "tradingview_webhook": "/api/webhook/tradingview"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(seconds_ago: i64) -> DateTime<Utc> {
        let now = Utc
            .with_ymd_and_hms(2026, 11, 9, 12, 0, 0)
            .single()
            .expect("valid datetime");
        now - chrono::Duration::seconds(seconds_ago)
    }

    #[test]
    fn feed_is_ok_requires_recent_scan_and_live_source() {
        let now = at(0);
        assert!(feed_is_ok("BINANCE_PAXG", Some(at(60)), now));
        assert!(feed_is_ok("TWELVEDATA", Some(at(180)), now));
        assert!(!feed_is_ok("BINANCE_PAXG", Some(at(181)), now));
        assert!(!feed_is_ok("BINANCE_PAXG", None, now));
        assert!(!feed_is_ok("FALLBACK_SEED", Some(at(1)), now));
        assert!(!feed_is_ok("SPOT_BASIS_UNAVAILABLE", Some(at(1)), now));
    }
}
