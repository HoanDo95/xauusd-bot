//! Point-in-time macro history for research/backtest.
//!
//! The live macro layer uses current FRED/Yahoo/CFTC snapshots. This module
//! provides a conservative historical view for backtests: each observation is
//! only visible after a fixed publication lag. Missing data returns `None`, not
//! a fabricated value. It never affects live signals.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const DEFAULT_LAG_DAYS: i64 = 1;

#[derive(Debug, Clone, Default)]
pub struct MacroSeries {
    points: Vec<(i64, f64)>,
    lag_ms: i64,
}

impl MacroSeries {
    pub fn from_points(points: Vec<(i64, f64)>, lag_days: i64) -> Self {
        let mut points: Vec<(i64, f64)> = points
            .into_iter()
            .filter(|(_, value)| value.is_finite())
            .collect();
        points.sort_by_key(|(timestamp, _)| *timestamp);
        Self {
            points,
            lag_ms: lag_days.max(0) * DAY_MS,
        }
    }

    /// Load a FRED CSV with `observation_date,<column>` and a conservative lag.
    pub fn load_csv(path: &Path, lag_days: i64) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|err| format!("read {}: {err}", path.display()))?;
        let mut points = Vec::new();
        for line in text.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split(',');
            let Some(date) = parts.next() else {
                continue;
            };
            let Some(value) = parts.next() else {
                continue;
            };
            let Ok(date) = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d") else {
                continue;
            };
            let Ok(value) = value.trim().parse::<f64>() else {
                continue;
            };
            let Some(datetime) = date.and_hms_opt(0, 0, 0) else {
                continue;
            };
            points.push((Utc.from_utc_datetime(&datetime).timestamp_millis(), value));
        }
        if points.is_empty() {
            return Err(format!("no usable points in {}", path.display()));
        }
        Ok(Self::from_points(points, lag_days))
    }

    pub fn value_at(&self, as_of_ms: i64) -> Option<f64> {
        let cutoff = as_of_ms.saturating_sub(self.lag_ms);
        let index = self
            .points
            .partition_point(|(timestamp, _)| *timestamp <= cutoff);
        index.checked_sub(1).map(|index| self.points[index].1)
    }

    pub fn value_days_before(&self, as_of_ms: i64, days: i64) -> Option<f64> {
        self.value_at(as_of_ms.saturating_sub(days.max(0) * DAY_MS))
    }

    pub fn change(&self, as_of_ms: i64, days: i64) -> Option<f64> {
        let current = self.value_at(as_of_ms)?;
        let previous = self.value_days_before(as_of_ms, days)?;
        Some(current - previous)
    }

    pub fn pct_change(&self, as_of_ms: i64, days: i64) -> Option<f64> {
        let current = self.value_at(as_of_ms)?;
        let previous = self.value_days_before(as_of_ms, days)?;
        if previous.abs() < f64::EPSILON {
            return None;
        }
        Some((current - previous) / previous * 100.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MacroPointInTime {
    pub as_of: String,
    pub real_yield: Option<f64>,
    pub real_yield_change_5d: Option<f64>,
    pub dollar: Option<f64>,
    pub dollar_change_5d_pct: Option<f64>,
    /// BULLISH | BEARISH | NEUTRAL | UNKNOWN
    pub bias: String,
    /// ALIGNED | COUNTER | MIXED | UNKNOWN
    pub alignment: String,
}

#[derive(Debug, Clone, Default)]
pub struct MacroHistory {
    real_yield: MacroSeries,
    dollar: MacroSeries,
}

impl MacroHistory {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let real_yield = MacroSeries::load_csv(&dir.join("DFII10.csv"), DEFAULT_LAG_DAYS)?;
        let dollar = MacroSeries::load_csv(&dir.join("DTWEXBGS.csv"), DEFAULT_LAG_DAYS)?;
        Ok(Self { real_yield, dollar })
    }

    pub fn at(&self, as_of_ms: i64, direction: &str) -> MacroPointInTime {
        let real_yield = self.real_yield.value_at(as_of_ms);
        let real_yield_change_5d = self.real_yield.change(as_of_ms, 5);
        let dollar = self.dollar.value_at(as_of_ms);
        let dollar_change_5d_pct = self.dollar.pct_change(as_of_ms, 5);
        let bias = classify_bias(real_yield_change_5d, dollar_change_5d_pct);
        let alignment = alignment_for(direction, &bias);
        MacroPointInTime {
            as_of: DateTime::<Utc>::from_timestamp_millis(as_of_ms)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            real_yield,
            real_yield_change_5d,
            dollar,
            dollar_change_5d_pct,
            bias,
            alignment,
        }
    }
}

fn classify_bias(real_yield_change_5d: Option<f64>, dollar_change_5d_pct: Option<f64>) -> String {
    let (Some(real_yield), Some(dollar)) = (real_yield_change_5d, dollar_change_5d_pct) else {
        return "UNKNOWN".to_string();
    };
    let yields_up = real_yield > 0.03;
    let yields_down = real_yield < -0.03;
    let dollar_up = dollar > 0.15;
    let dollar_down = dollar < -0.15;
    if yields_up && dollar_up {
        "BEARISH".to_string()
    } else if yields_down && dollar_down {
        "BULLISH".to_string()
    } else {
        "NEUTRAL".to_string()
    }
}

fn alignment_for(direction: &str, bias: &str) -> String {
    let direction = direction.trim().to_uppercase();
    match (direction.as_str(), bias) {
        ("SELL", "BEARISH") | ("BUY", "BULLISH") => "ALIGNED".to_string(),
        ("SELL", "BULLISH") | ("BUY", "BEARISH") => "COUNTER".to_string(),
        ("SELL" | "BUY", "NEUTRAL") => "MIXED".to_string(),
        _ => "UNKNOWN".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ms(year: i32, month: u32, day: u32) -> i64 {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn value_at_respects_publication_lag() {
        let series =
            MacroSeries::from_points(vec![(ms(2026, 1, 1), 1.0), (ms(2026, 1, 2), 2.0)], 1);
        // On Jan 2, the Jan 2 observation is not published yet; Jan 1 is.
        assert_eq!(series.value_at(ms(2026, 1, 2)), Some(1.0));
        // On Jan 3, the Jan 2 observation is visible.
        assert_eq!(series.value_at(ms(2026, 1, 3)), Some(2.0));
    }

    #[test]
    fn change_and_pct_change_use_point_in_time_values() {
        let series = MacroSeries::from_points(
            vec![
                (ms(2026, 1, 1), 10.0),
                (ms(2026, 1, 2), 11.0),
                (ms(2026, 1, 7), 12.0),
            ],
            0,
        );
        assert_eq!(series.change(ms(2026, 1, 7), 5), Some(1.0));
        assert!((series.pct_change(ms(2026, 1, 7), 5).unwrap() - 9.0909090909).abs() < 1e-6);
    }

    #[test]
    fn bias_and_alignment_are_deterministic() {
        assert_eq!(classify_bias(Some(0.1), Some(0.5)), "BEARISH");
        assert_eq!(classify_bias(Some(-0.1), Some(-0.5)), "BULLISH");
        assert_eq!(classify_bias(Some(0.1), Some(-0.5)), "NEUTRAL");
        assert_eq!(classify_bias(None, Some(0.5)), "UNKNOWN");
        assert_eq!(alignment_for("SELL", "BEARISH"), "ALIGNED");
        assert_eq!(alignment_for("BUY", "BEARISH"), "COUNTER");
        assert_eq!(alignment_for("SELL", "BULLISH"), "COUNTER");
        assert_eq!(alignment_for("BUY", "NEUTRAL"), "MIXED");
    }

    #[test]
    fn history_loads_fred_csv_fixtures() {
        let dir = std::env::temp_dir().join(format!("macro_history_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(
            dir.join("DFII10.csv"),
            "observation_date,DFII10\n2026-01-01,1.0\n2026-01-02,1.1\n",
        )
        .unwrap();
        fs::write(
            dir.join("DTWEXBGS.csv"),
            "observation_date,DTWEXBGS\n2026-01-01,100.0\n2026-01-02,101.0\n",
        )
        .unwrap();
        let history = MacroHistory::load(&dir).expect("load history");
        let point = history.at(ms(2026, 1, 4), "SELL");
        assert!(point.real_yield.is_some());
        assert!(point.dollar.is_some());
        assert!(matches!(
            point.bias.as_str(),
            "BULLISH" | "BEARISH" | "NEUTRAL" | "UNKNOWN"
        ));
        let _ = fs::remove_dir_all(dir);
    }
}
