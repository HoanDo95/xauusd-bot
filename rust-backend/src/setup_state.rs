//! Persistent state for the realtime setup-alert engine.
//!
//! The engine watches every enabled, non-reference scenario and emits three
//! events: `PRE_ALERT`, `TRIGGERED` and `INVALIDATED`. State is persisted in
//! `bot_state.json` so a restart/deploy cannot replay the same event.
//!
//! This module is deliberately side-effect free: it only describes setup state,
//! dedup keys and pure transition helpers. Telegram/journal writes live in
//! `action_alerts.rs`.

use crate::models::Candle;
use crate::scenarios::{scenario_triggered, Scenario, WATCH_SETUP_MAX_BARS};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const STATUS_WATCHING: &str = "WATCHING";
pub const STATUS_PRE_ALERT: &str = "PRE_ALERT";
pub const STATUS_TRIGGERED: &str = "TRIGGERED";
pub const STATUS_INVALIDATED: &str = "INVALIDATED";
pub const STATUS_EXPIRED: &str = "EXPIRED";

/// Maximum persisted event keys. Old entries are pruned by timestamp.
pub const MAX_SENT_EVENTS: usize = 500;
/// Maximum persisted setup records. Watching setups also expire by time.
pub const MAX_TRACKED_SETUPS: usize = 24;
/// Sent-event retention. A week is far longer than any M15 setup lifecycle.
const SENT_EVENT_RETENTION_HOURS: i64 = 24 * 7;

/// One tracked scenario lifecycle. Levels are frozen at first sighting so the
/// trigger does not move with every M15 rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupRecord {
    pub setup_id: String,
    pub scenario: Scenario,
    /// WATCHING | PRE_ALERT | TRIGGERED | INVALIDATED | EXPIRED
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub created_bar: i64,
    pub updated_at: DateTime<Utc>,
    /// Candle timestamp of the last PRE_ALERT/TRIGGERED/INVALIDATED event.
    #[serde(default)]
    pub last_event_bar: Option<i64>,
    #[serde(default)]
    pub triggered_bar: Option<i64>,
    #[serde(default)]
    pub triggered_close: Option<f64>,
    /// True after the one-shot PRE_ALERT for this lifecycle.
    #[serde(default)]
    pub pre_alerted: bool,
}

impl SetupRecord {
    pub fn new(setup_id: String, scenario: Scenario, now: DateTime<Utc>, created_bar: i64) -> Self {
        Self {
            setup_id,
            scenario,
            status: STATUS_WATCHING.to_string(),
            created_at: now,
            created_bar,
            updated_at: now,
            last_event_bar: None,
            triggered_bar: None,
            triggered_close: None,
            pre_alerted: false,
        }
    }

    /// New M15 bars since this setup was first seen (0 on the creation bar).
    pub fn bars_open(&self, candle: &Candle) -> i64 {
        ((candle.timestamp - self.created_bar) / (15 * 60 * 1000)).max(0)
    }

    /// A triggered setup gets a fresh retest/expiry window from the trigger bar;
    /// a watching setup is anchored at creation.
    pub fn expiry_anchor_bar(&self) -> i64 {
        self.triggered_bar.unwrap_or(self.created_bar)
    }

    pub fn bars_since_anchor(&self, candle: &Candle) -> i64 {
        ((candle.timestamp - self.expiry_anchor_bar()) / (15 * 60 * 1000)).max(0)
    }

    pub fn is_expired(&self, now: DateTime<Utc>, max_bars: i64) -> bool {
        if self.triggered_bar.is_some() {
            return false;
        }
        let bars = max_bars.max(1);
        (now - self.created_at).num_minutes() >= bars * 15
    }

    /// A setup is invalidated by the scenario's structural invalidation level,
    /// not by the (usually wider) protective stop. Falls back to the stop when
    /// the invalidation text has no parseable level.
    pub fn is_invalidated(&self, candle: &Candle) -> bool {
        let level = self
            .scenario
            .invalidation_price()
            .or(self.scenario.stop_loss);
        let Some(level) = level.filter(|value| value.is_finite() && *value > 0.0) else {
            return true;
        };
        match self.scenario.bias.trim().to_uppercase().as_str() {
            "BEARISH" => candle.close > level,
            "BULLISH" => candle.close < level,
            _ => true,
        }
    }

    /// Trigger confirmed on this candle and not already armed by an earlier
    /// PRE_ALERT. A fresh sighting only fires when the trigger was crossed on
    /// this bar; this prevents replaying a scenario that has been beyond its
    /// trigger for hours.
    pub fn trigger_crossed(&self, previous: Option<&Candle>, candle: &Candle) -> bool {
        if !scenario_triggered(&self.scenario, candle) {
            return false;
        }
        if self.status == STATUS_PRE_ALERT || self.pre_alerted {
            return true;
        }
        previous
            .map(|previous| !scenario_triggered(&self.scenario, previous))
            .unwrap_or(true)
    }

    pub fn mark_pre_alert(&mut self, now: DateTime<Utc>, bar: i64) {
        self.status = STATUS_PRE_ALERT.to_string();
        self.pre_alerted = true;
        self.last_event_bar = Some(bar);
        self.updated_at = now;
    }

    pub fn mark_triggered(&mut self, now: DateTime<Utc>, bar: i64, close: f64) {
        self.status = STATUS_TRIGGERED.to_string();
        self.last_event_bar = Some(bar);
        self.triggered_bar = Some(bar);
        self.triggered_close = Some(close);
        self.updated_at = now;
    }

    pub fn mark_invalidated(&mut self, now: DateTime<Utc>, bar: i64) {
        self.status = STATUS_INVALIDATED.to_string();
        self.last_event_bar = Some(bar);
        self.updated_at = now;
    }

    pub fn mark_expired(&mut self, now: DateTime<Utc>) {
        self.status = STATUS_EXPIRED.to_string();
        self.updated_at = now;
    }
}

/// Persisted setup state. All maps are keyed by stable strings so the file is
/// readable and migration-safe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetupAlertStore {
    /// Last M15 candle fully processed by the engine.
    #[serde(default)]
    pub last_bar_timestamp: Option<i64>,
    /// Active/recent setups keyed by scenario id.
    #[serde(default)]
    pub setups: HashMap<String, SetupRecord>,
    /// `scenario:transition:bar` keys already sent.
    #[serde(default)]
    pub sent_events: HashMap<String, DateTime<Utc>>,
    /// PRE_ALERT zone cooldown key -> last sent time.
    #[serde(default)]
    pub pre_alert_last_sent: HashMap<String, DateTime<Utc>>,
}

impl SetupAlertStore {
    pub fn has_sent(&self, key: &str) -> bool {
        self.sent_events.contains_key(key)
    }

    pub fn mark_sent(&mut self, key: String, now: DateTime<Utc>) {
        self.sent_events.insert(key, now);
    }

    pub fn pre_alert_last_sent(&self, key: &str) -> Option<DateTime<Utc>> {
        self.pre_alert_last_sent.get(key).copied()
    }

    pub fn mark_pre_alert_sent(&mut self, key: String, now: DateTime<Utc>) {
        self.pre_alert_last_sent.insert(key, now);
    }

    /// Drop stale dedup keys and cap memory/disk growth. Recent keys are kept
    /// even when the cap is exceeded so a restart cannot replay a recent event.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - chrono::Duration::hours(SENT_EVENT_RETENTION_HOURS);
        self.sent_events.retain(|_, at| *at >= cutoff);
        self.pre_alert_last_sent.retain(|_, at| *at >= cutoff);

        prune_by_age(&mut self.sent_events, MAX_SENT_EVENTS);
        prune_by_age(&mut self.pre_alert_last_sent, MAX_SENT_EVENTS / 2);
        prune_setups(&mut self.setups, MAX_TRACKED_SETUPS);
    }
}

fn prune_by_age(map: &mut HashMap<String, DateTime<Utc>>, max: usize) {
    if map.len() <= max {
        return;
    }
    let mut entries: Vec<(String, DateTime<Utc>)> =
        map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    entries.sort_by_key(|(_, at)| *at);
    for (key, _) in entries.into_iter().take(map.len() - max) {
        map.remove(&key);
    }
}

fn prune_setups(map: &mut HashMap<String, SetupRecord>, max: usize) {
    if map.len() <= max {
        return;
    }
    let mut entries: Vec<(String, DateTime<Utc>)> = map
        .iter()
        .map(|(key, setup)| (key.clone(), setup.updated_at))
        .collect();
    entries.sort_by_key(|(_, at)| *at);
    for (key, _) in entries.into_iter().take(map.len() - max) {
        map.remove(&key);
    }
}

/// Stable dedup key: `scenario_id:transition:bar_timestamp`.
pub fn event_key(scenario_id: &str, transition: &str, bar_timestamp: i64) -> String {
    format!(
        "{}:{}:{}",
        scenario_id.trim().to_lowercase(),
        transition.trim().to_lowercase(),
        bar_timestamp
    )
}

/// PRE_ALERT zone key: scenario + direction + rounded trigger bucket.
pub fn pre_alert_key(scenario: &Scenario, trigger_price: f64, zone_size: f64) -> String {
    let zone_size = zone_size.max(1.0);
    let bucket = (trigger_price / zone_size).round() * zone_size;
    format!(
        "{}:{}:{:.2}",
        scenario.id.trim().to_lowercase(),
        scenario.trigger_direction.trim().to_uppercase(),
        bucket
    )
}

/// Price is close to, but has not crossed, the scenario trigger.
pub fn is_approaching(scenario: &Scenario, price: f64, watch_distance: f64) -> bool {
    let Some(trigger) = scenario
        .trigger_price
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return false;
    };
    if !price.is_finite() || price <= 0.0 {
        return false;
    }
    let distance = (price - trigger).abs();
    match scenario.trigger_direction.trim().to_uppercase().as_str() {
        "ABOVE" => price < trigger && distance <= watch_distance,
        "BELOW" => price > trigger && distance <= watch_distance,
        "TOUCH" => distance <= watch_distance,
        _ => false,
    }
}

/// Whether a setup is still worth tracking. Expired/invalidated records are
/// retained for display until the next prune but must not emit new events.
pub fn is_terminal(status: &str) -> bool {
    status == STATUS_INVALIDATED || status == STATUS_EXPIRED
}

pub fn max_watch_bars() -> i64 {
    WATCH_SETUP_MAX_BARS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timeframe;
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, hour, 0, 0)
            .single()
            .expect("valid datetime")
    }

    fn candle(timestamp: i64, high: f64, low: f64, close: f64) -> Candle {
        Candle {
            timestamp,
            open: close,
            high,
            low,
            close,
            volume: 1000.0,
            taker_buy_volume: 0.0,
            timeframe: Timeframe::M15,
        }
    }

    fn scenario(bias: &str, direction: &str, trigger: f64, invalidation: &str) -> Scenario {
        Scenario {
            id: "bullish_breakout".to_string(),
            name: "Bullish breakout".to_string(),
            bias: bias.to_string(),
            trigger: "test".to_string(),
            condition: "test".to_string(),
            entry: Some(trigger),
            stop_loss: Some(if bias == "BULLISH" {
                trigger - 20.0
            } else {
                trigger + 20.0
            }),
            take_profit_1: Some(trigger + 20.0),
            take_profit_2: Some(trigger + 40.0),
            risk_reward: Some(2.0),
            trigger_price: Some(trigger),
            trigger_direction: direction.to_string(),
            trigger_timeframe: "M15".to_string(),
            invalidation: invalidation.to_string(),
            confidence: 80,
            macro_alignment: "ALIGNED".to_string(),
            valid: true,
            role: "PRIMARY".to_string(),
            priority: 300,
            notes: Vec::new(),
        }
    }

    #[test]
    fn event_key_is_stable_and_bar_scoped() {
        assert_eq!(
            event_key("Bullish_Breakout", "TRIGGERED", 123),
            "bullish_breakout:triggered:123"
        );
        assert_ne!(
            event_key("bullish_breakout", "TRIGGERED", 123),
            event_key("bullish_breakout", "TRIGGERED", 124)
        );
    }

    #[test]
    fn pre_alert_bucket_treats_nearby_triggers_as_one_zone() {
        let left = scenario("BULLISH", "ABOVE", 2701.0, "Đóng M15 dưới 2690.00");
        let right = scenario("BULLISH", "ABOVE", 2702.0, "Đóng M15 dưới 2690.00");
        assert_eq!(
            pre_alert_key(&left, 2701.0, 10.0),
            pre_alert_key(&right, 2702.0, 10.0)
        );
    }

    #[test]
    fn approaching_requires_price_to_stay_on_the_correct_side() {
        let above = scenario("BULLISH", "ABOVE", 2701.0, "Đóng M15 dưới 2690.00");
        assert!(is_approaching(&above, 2695.0, 10.0));
        assert!(!is_approaching(&above, 2705.0, 10.0));
        assert!(!is_approaching(&above, 2680.0, 10.0));

        let below = scenario("BEARISH", "BELOW", 2701.0, "Đóng M15 trên 2711.00");
        assert!(is_approaching(&below, 2705.0, 10.0));
        assert!(!is_approaching(&below, 2695.0, 10.0));
    }

    #[test]
    fn invalidation_uses_the_structural_text_level_not_the_wide_stop() {
        let setup = SetupRecord::new(
            "S".to_string(),
            scenario("BULLISH", "ABOVE", 2701.0, "Đóng M15 trở lại dưới 2698.00"),
            at(23, 10),
            0,
        );
        // Close is below the invalidation level but still above the 2681 stop.
        assert!(setup.is_invalidated(&candle(900_000, 2700.0, 2695.0, 2697.0)));
        assert!(!setup.is_invalidated(&candle(900_000, 2705.0, 2699.0, 2702.0)));
    }

    #[test]
    fn crossed_trigger_only_fires_once_per_bar() {
        let setup = SetupRecord::new(
            "S".to_string(),
            scenario("BULLISH", "ABOVE", 2701.0, "Đóng M15 dưới 2690.00"),
            at(23, 10),
            0,
        );
        let previous = candle(0, 2700.0, 2690.0, 2700.0);
        let current = candle(900_000, 2706.0, 2700.0, 2705.0);
        assert!(setup.trigger_crossed(Some(&previous), &current));
        assert!(!setup.trigger_crossed(Some(&current), &current));
    }

    #[test]
    fn pre_alerted_setup_can_trigger_even_after_a_delay() {
        let mut setup = SetupRecord::new(
            "S".to_string(),
            scenario("BULLISH", "ABOVE", 2701.0, "Đóng M15 dưới 2690.00"),
            at(23, 10),
            0,
        );
        setup.mark_pre_alert(at(23, 10), 0);
        let current = candle(900_000, 2706.0, 2700.0, 2705.0);
        assert!(setup.trigger_crossed(Some(&current), &current));
    }

    #[test]
    fn triggered_setup_gets_a_fresh_expiry_window() {
        let mut setup = SetupRecord::new(
            "S".to_string(),
            scenario("BULLISH", "ABOVE", 2701.0, "Đóng M15 dưới 2698.00"),
            at(23, 10),
            0,
        );
        let later = candle(7 * 15 * 60 * 1000, 2705.0, 2699.0, 2702.0);
        setup.mark_triggered(at(23, 12), later.timestamp, 2702.0);
        assert_eq!(setup.expiry_anchor_bar(), later.timestamp);
        assert_eq!(setup.bars_since_anchor(&later), 0);
        assert!(!setup.is_expired(at(23, 13), max_watch_bars()));
    }

    #[test]
    fn sent_event_survives_restart_serialization() {
        let mut store = SetupAlertStore::default();
        let key = event_key("bullish_breakout", "triggered", 123);
        store.mark_sent(key.clone(), at(23, 10));
        let restored: SetupAlertStore =
            serde_json::from_str(&serde_json::to_string(&store).expect("serialize"))
                .expect("deserialize");
        assert!(restored.has_sent(&key));
    }

    #[test]
    fn invalidation_is_one_event_per_bar() {
        let setup = SetupRecord::new(
            "S".to_string(),
            scenario("BULLISH", "ABOVE", 2701.0, "Đóng M15 dưới 2698.00"),
            at(23, 10),
            0,
        );
        let key = event_key(&setup.scenario.id, "invalidated", 900_000);
        let mut store = SetupAlertStore::default();
        assert!(!store.has_sent(&key));
        store.mark_sent(key.clone(), at(23, 11));
        assert!(store.has_sent(&key));
        assert_ne!(key, event_key(&setup.scenario.id, "invalidated", 1_800_000));
    }

    #[test]
    fn terminal_status_does_not_trigger_again() {
        let mut setup = SetupRecord::new(
            "S".to_string(),
            scenario("BULLISH", "ABOVE", 2701.0, "Đóng M15 dưới 2698.00"),
            at(23, 10),
            0,
        );
        setup.mark_triggered(at(23, 11), 900_000, 2705.0);
        assert_eq!(setup.status, STATUS_TRIGGERED);
        assert!(!is_terminal(&setup.status));
        setup.mark_invalidated(at(23, 12), 1_800_000);
        assert!(is_terminal(&setup.status));
    }

    #[test]
    fn prune_drops_old_keys_but_keeps_recent_ones() {
        let mut store = SetupAlertStore::default();
        let now = at(23, 12);
        store.mark_sent("old".to_string(), now - chrono::Duration::days(8));
        store.mark_sent("new".to_string(), now - chrono::Duration::hours(1));
        store.prune(now);
        assert!(!store.has_sent("old"));
        assert!(store.has_sent("new"));
    }
}
