//! Daily plan anchor: one frozen scenario set per VN slot (07:00, 13:00, 19:00).
//!
//! The daily view and the scanner share the same levels for a slot. A refresh
//! must not re-anchor the plan to the latest price; only a new slot (or an
//! explicit invalidation) may replace it.

use crate::scenarios::ScenarioResponse;
use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::warn;

pub const SLOT_HOURS_VN: [u32; 3] = [7, 13, 19];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyPlan {
    /// `YYYY-MM-DD:HH` in Vietnam time.
    pub slot_key: String,
    pub generated_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub scenarios: ScenarioResponse,
}

#[derive(Debug, Clone)]
pub struct DailyPlanStore {
    path: PathBuf,
}

impl DailyPlanStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_env() -> Self {
        let path =
            std::env::var("DAILY_PLAN_PATH").unwrap_or_else(|_| "data/daily_plan.json".to_string());
        Self::new(path)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn load(&self) -> Option<DailyPlan> {
        let content = fs::read_to_string(&self.path).ok()?;
        match serde_json::from_str::<DailyPlan>(&content) {
            Ok(plan) => Some(plan),
            Err(err) => {
                warn!(
                    "⚠️ [DailyPlan] File {} không hợp lệ ({}), bỏ qua.",
                    self.path.display(),
                    err
                );
                None
            }
        }
    }

    pub fn save(&self, plan: &DailyPlan) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let temp = self.path.with_extension("json.tmp");
        let payload = serde_json::to_string_pretty(plan).map_err(|err| err.to_string())?;
        fs::write(&temp, payload).map_err(|err| err.to_string())?;
        fs::rename(&temp, &self.path).map_err(|err| err.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSlot {
    pub key: String,
    pub generated_at: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
}

/// Latest weekday slot at or before `now` (Vietnam time).
pub fn current_slot(now: DateTime<Utc>) -> PlanSlot {
    let vn = now + Duration::hours(7);
    for day_offset in 0..=10 {
        let date = (vn - Duration::days(day_offset)).date_naive();
        if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            continue;
        }
        for hour in [19u32, 13, 7] {
            let Some(local) = date.and_hms_opt(hour, 0, 0) else {
                continue;
            };
            let generated_at = Utc.from_utc_datetime(&local) - Duration::hours(7);
            if generated_at <= now {
                return PlanSlot {
                    key: format!("{}:{:02}", date.format("%Y-%m-%d"), hour),
                    generated_at,
                    valid_until: next_slot(generated_at),
                };
            }
        }
    }
    PlanSlot {
        key: format!("{}:{:02}", vn.format("%Y-%m-%d"), vn.hour()),
        generated_at: now,
        valid_until: now + Duration::hours(1),
    }
}

/// Next weekday daily slot strictly after `after` (Vietnam time).
pub fn next_slot(after: DateTime<Utc>) -> DateTime<Utc> {
    let vn = after + Duration::hours(7);
    for day_offset in 0..=10 {
        let date = (vn + Duration::days(day_offset)).date_naive();
        if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            continue;
        }
        for hour in SLOT_HOURS_VN {
            let Some(local) = date.and_hms_opt(hour, 0, 0) else {
                continue;
            };
            let candidate = Utc.from_utc_datetime(&local) - Duration::hours(7);
            if candidate > after {
                return candidate;
            }
        }
    }
    after + Duration::hours(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap()
    }

    #[test]
    fn weekday_morning_uses_previous_slot() {
        // Monday 06:00 VN = Sunday 23:00 UTC -> Friday 19:00 VN slot.
        let slot = current_slot(utc(2026, 11, 22, 23, 0));
        assert_eq!(slot.key, "2026-11-20:19");
        assert_eq!(slot.valid_until, utc(2026, 11, 23, 0, 0)); // Monday 07:00 VN
    }

    #[test]
    fn monday_after_open_uses_monday_slot() {
        // Monday 07:01 VN = Monday 00:01 UTC.
        let slot = current_slot(utc(2026, 11, 23, 0, 1));
        assert_eq!(slot.key, "2026-11-23:07");
        assert_eq!(slot.valid_until, utc(2026, 11, 23, 6, 0)); // Monday 13:00 VN
    }

    #[test]
    fn friday_evening_holds_until_monday() {
        // Friday 20:00 VN = Friday 13:00 UTC.
        let slot = current_slot(utc(2026, 11, 20, 13, 0));
        assert_eq!(slot.key, "2026-11-20:19");
        assert_eq!(slot.valid_until, utc(2026, 11, 23, 0, 0));
    }

    #[test]
    fn saturday_uses_friday_plan() {
        // Saturday 10:00 VN = Saturday 03:00 UTC.
        let slot = current_slot(utc(2026, 11, 21, 3, 0));
        assert_eq!(slot.key, "2026-11-20:19");
    }
}
