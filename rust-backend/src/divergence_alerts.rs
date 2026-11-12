//! Standalone RSI divergence alerts for M15/H1/H4.
//!
//! This module is intentionally independent from the trade-signal pipeline:
//! it never consumes the daily Sniper quota, never inserts into `signals`, and
//! never touches entry/SL/TP logic. Alerts are advisory momentum warnings only
//! and are kept in memory (not persisted to disk).

use crate::api::AppState;
use crate::rsi::{divergence_label, rsi_snapshot, RsiDivergence};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};

/// Maximum number of in-memory divergence alerts kept for the dashboard.
pub const MAX_DIVERGENCE_ALERTS: usize = 50;
/// Worker polling cadence.
const DIVERGENCE_INTERVAL_SECS: u64 = 60;

/// One RSI/price divergence alert. Advisory only — no entry, SL or TP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DivergenceAlert {
    pub id: String,
    pub symbol: String,
    /// "M15" | "H1" | "H4"
    pub timeframe: String,
    /// "Bullish" | "Bearish"
    pub direction: String,
    pub price: f64,
    pub rsi: f64,
    pub candle_timestamp: i64,
    pub detected_at: DateTime<Utc>,
    /// Short Vietnamese explanation.
    pub note: String,
}

/// Short Vietnamese explanation for a divergence direction.
pub fn divergence_note(direction: &str) -> &'static str {
    match direction.trim().to_uppercase().as_str() {
        "BEARISH" => "Giá tạo đỉnh cao hơn nhưng RSI tạo đỉnh thấp hơn.",
        "BULLISH" => "Giá tạo đáy thấp hơn nhưng RSI tạo đáy cao hơn.",
        _ => "Phân kỳ RSI so với giá — chỉ là cảnh báo động lượng.",
    }
}

/// Decide whether a new alert may be sent for one timeframe + direction.
///
/// `last_sent` is keyed by `"{timeframe}:{direction}"`. Cooldowns below 5
/// minutes are treated as 5 minutes to avoid Telegram spam.
pub fn should_alert(
    enabled: bool,
    timeframe_enabled: bool,
    last_sent: Option<DateTime<Utc>>,
    cooldown_minutes: i64,
    now: DateTime<Utc>,
) -> bool {
    if !enabled || !timeframe_enabled {
        return false;
    }

    match last_sent {
        None => true,
        Some(last_sent) => (now - last_sent).num_minutes() >= cooldown_minutes.max(5),
    }
}

/// Build an alert from a detected divergence. Returns `None` for no divergence
/// or invalid price/RSI values so malformed snapshots never reach Telegram.
pub fn build_alert(
    symbol: &str,
    timeframe: &str,
    direction: RsiDivergence,
    price: f64,
    rsi: f64,
    candle_timestamp: i64,
    now: DateTime<Utc>,
) -> Option<DivergenceAlert> {
    if direction == RsiDivergence::None {
        return None;
    }
    if !price.is_finite() || price <= 0.0 {
        return None;
    }
    if !rsi.is_finite() || rsi < 0.0 || rsi > 100.0 {
        return None;
    }

    let direction = divergence_label(direction);
    let timeframe = timeframe.trim().to_uppercase();
    let id = format!(
        "DIV-{}-{}-{}-{}",
        timeframe,
        direction,
        candle_timestamp,
        now.timestamp_millis()
    );

    Some(DivergenceAlert {
        id,
        symbol: symbol.to_string(),
        timeframe,
        direction: direction.clone(),
        price,
        rsi,
        candle_timestamp,
        detected_at: now,
        note: divergence_note(&direction).to_string(),
    })
}

/// Spawn the standalone divergence alert worker (60-second cadence).
///
/// M15 is computed from `state.candles` with fixed 14/70/30/40/2 parameters.
/// H1/H4 reuse the RSI snapshots already attached to `state.market_context`.
pub fn spawn_divergence_worker(state: AppState) {
    tokio::spawn(async move {
        info!(
            "📐 [Divergence Alerts] Worker cảnh báo phân kỳ RSI M15/H1/H4 đã khởi chạy (chu kỳ {} giây)...",
            DIVERGENCE_INTERVAL_SECS
        );
        let mut interval = tokio::time::interval(Duration::from_secs(DIVERGENCE_INTERVAL_SECS));

        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.divergence_alert_enabled {
                continue;
            }

            // Never alert on synthetic/stale fallback candles.
            let active_data_source = state.active_data_source.read().await.clone();
            if active_data_source.eq_ignore_ascii_case("FALLBACK_SEED")
                || active_data_source.eq_ignore_ascii_case("SPOT_BASIS_UNAVAILABLE")
            {
                continue;
            }

            // (timeframe, direction, price, rsi, candle_timestamp)
            let mut candidates: Vec<(String, RsiDivergence, f64, f64, i64)> = Vec::new();

            {
                let candles = state.candles.read().await;
                if let Some(last) = candles.last() {
                    let snapshot = rsi_snapshot(&candles, 14, 70.0, 30.0, 40, 2);
                    if let Some(value) = snapshot.value {
                        candidates.push((
                            "M15".to_string(),
                            snapshot.divergence,
                            last.close,
                            value,
                            last.timestamp,
                        ));
                    }
                }
            }

            {
                let context = state.market_context.read().await;
                if let Some(context) = context.as_ref() {
                    if let Some(snapshot) = context.rsi_h1.as_ref() {
                        if let Some(value) = snapshot.value {
                            candidates.push((
                                "H1".to_string(),
                                snapshot.divergence,
                                context.h1.close,
                                value,
                                context.h1.last_candle_timestamp,
                            ));
                        }
                    }
                    if let Some(snapshot) = context.rsi_h4.as_ref() {
                        if let Some(value) = snapshot.value {
                            candidates.push((
                                "H4".to_string(),
                                snapshot.divergence,
                                context.h4.close,
                                value,
                                context.h4.last_candle_timestamp,
                            ));
                        }
                    }
                }
            }

            let now = Utc::now();
            for (timeframe, direction, price, rsi, candle_timestamp) in candidates {
                if direction == RsiDivergence::None {
                    continue;
                }

                let timeframe_enabled = match timeframe.as_str() {
                    "M15" => config.divergence_alert_m15,
                    "H1" => config.divergence_alert_h1,
                    "H4" => config.divergence_alert_h4,
                    _ => false,
                };
                let key = format!("{}:{}", timeframe, divergence_label(direction));
                let last_sent = state.divergence_last_sent.read().await.get(&key).copied();

                if !should_alert(
                    config.divergence_alert_enabled,
                    timeframe_enabled,
                    last_sent,
                    config.divergence_alert_cooldown_minutes,
                    now,
                ) {
                    continue;
                }

                let Some(alert) = build_alert(
                    &config.symbol,
                    &timeframe,
                    direction,
                    price,
                    rsi,
                    candle_timestamp,
                    now,
                ) else {
                    continue;
                };

                let telegram = state.telegram.read().await.clone();
                match telegram.send_divergence_alert(&alert).await {
                    Ok(msg_id) => {
                        info!(
                            "🔔 [Divergence] {} {} · RSI {:.1} · giá {:.2} · msg_id={}",
                            alert.timeframe, alert.direction, alert.rsi, alert.price, msg_id
                        );
                        state
                            .divergence_last_sent
                            .write()
                            .await
                            .insert(key, alert.detected_at);
                        let mut alerts = state.divergence_alerts.write().await;
                        alerts.insert(0, alert);
                        alerts.truncate(MAX_DIVERGENCE_ALERTS);
                    }
                    Err(err) => {
                        warn!(
                            "⚠️ [Divergence] Không gửi được cảnh báo {} {}: {}",
                            alert.timeframe, alert.direction, err
                        );
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 12, 12, 0, 0)
            .single()
            .expect("valid datetime")
    }

    #[test]
    fn divergence_note_matches_both_directions() {
        assert_eq!(
            divergence_note("Bearish"),
            "Giá tạo đỉnh cao hơn nhưng RSI tạo đỉnh thấp hơn."
        );
        assert_eq!(
            divergence_note("bullish"),
            "Giá tạo đáy thấp hơn nhưng RSI tạo đáy cao hơn."
        );
        assert!(divergence_note("unknown").contains("cảnh báo động lượng"));
    }

    #[test]
    fn should_alert_respects_enabled_flags_and_missing_last_sent() {
        let now = fixed_now();

        assert!(!should_alert(false, true, None, 60, now));
        assert!(!should_alert(true, false, None, 60, now));
        assert!(!should_alert(false, false, None, 60, now));
        assert!(should_alert(true, true, None, 60, now));
    }

    #[test]
    fn should_alert_enforces_cooldown_boundary() {
        let now = fixed_now();
        let last = now - chrono::Duration::minutes(59);
        assert!(!should_alert(true, true, Some(last), 60, now));

        let last = now - chrono::Duration::minutes(60);
        assert!(should_alert(true, true, Some(last), 60, now));

        let last = now - chrono::Duration::minutes(61);
        assert!(should_alert(true, true, Some(last), 60, now));
    }

    #[test]
    fn should_alert_clamps_cooldown_to_five_minutes() {
        let now = fixed_now();
        let last = now - chrono::Duration::minutes(4);
        assert!(!should_alert(true, true, Some(last), 0, now));

        let last = now - chrono::Duration::minutes(5);
        assert!(should_alert(true, true, Some(last), 0, now));
    }

    #[test]
    fn should_alert_blocks_clock_skew() {
        let now = fixed_now();
        let last = now + chrono::Duration::minutes(5);
        assert!(!should_alert(true, true, Some(last), 60, now));
    }

    #[test]
    fn build_alert_returns_none_without_divergence_or_with_invalid_values() {
        let now = fixed_now();

        assert!(build_alert("XAU/USD", "M15", RsiDivergence::None, 2400.0, 55.0, 1, now).is_none());

        for price in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                build_alert(
                    "XAU/USD",
                    "M15",
                    RsiDivergence::Bearish,
                    price,
                    55.0,
                    1,
                    now
                )
                .is_none(),
                "price {price} must be rejected"
            );
        }

        for rsi in [-1.0, 100.1, f64::NAN, f64::INFINITY] {
            assert!(
                build_alert(
                    "XAU/USD",
                    "M15",
                    RsiDivergence::Bullish,
                    2400.0,
                    rsi,
                    1,
                    now
                )
                .is_none(),
                "rsi {rsi} must be rejected"
            );
        }
    }

    #[test]
    fn build_alert_accepts_rsi_zero_as_valid_oversold_extreme() {
        let now = fixed_now();
        let alert = build_alert(
            "XAU/USD",
            "M15",
            RsiDivergence::Bullish,
            2400.0,
            0.0,
            1,
            now,
        )
        .expect("RSI 0 is a valid extreme value");

        assert_eq!(alert.rsi, 0.0);
    }

    #[test]
    fn build_alert_populates_dashboard_and_telegram_fields() {
        let now = fixed_now();
        let alert = build_alert(
            "XAU/USD",
            "h4",
            RsiDivergence::Bearish,
            2412.34,
            68.2,
            1_700_000_000_000,
            now,
        )
        .expect("valid alert");

        assert!(alert.id.starts_with("DIV-H4-Bearish-1700000000000-"));
        assert_eq!(alert.symbol, "XAU/USD");
        assert_eq!(alert.timeframe, "H4");
        assert_eq!(alert.direction, "Bearish");
        assert_eq!(alert.price, 2412.34);
        assert_eq!(alert.rsi, 68.2);
        assert_eq!(alert.candle_timestamp, 1_700_000_000_000);
        assert_eq!(alert.detected_at, now);
        assert_eq!(
            alert.note,
            "Giá tạo đỉnh cao hơn nhưng RSI tạo đỉnh thấp hơn."
        );
    }

    #[test]
    fn build_alert_accepts_bullish_direction() {
        let alert = build_alert(
            "XAU/USD",
            "H1",
            RsiDivergence::Bullish,
            2380.0,
            32.5,
            42,
            fixed_now(),
        )
        .expect("valid bullish alert");

        assert_eq!(alert.direction, "Bullish");
        assert_eq!(
            alert.note,
            "Giá tạo đáy thấp hơn nhưng RSI tạo đáy cao hơn."
        );
    }
}
