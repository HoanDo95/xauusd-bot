//! Read-only Exness MT5 bar feed.
//!
//! The Exness MT5 terminal (Wine) runs an MQL5 exporter that writes
//! `MQL5/Files/xauusd_bars.json`. A host-side sync copies that file to
//! `data/exness_bars.json`, which is mounted into the API container. This module
//! parses the file and converts it to `Candle` vectors for the existing feed.
//!
//! It never places orders and never talks to the broker directly.

use crate::models::{Candle, Timeframe};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::warn;

pub const SOURCE_EXNESS: &str = "EXNESS";
pub const VOLUME_SOURCE_EXNESS_TICK: &str = "EXNESS_TICK";
pub const DEFAULT_EXNESS_PATH: &str = "data/exness_bars.json";
/// The EA writes every 60s; allow up to 3 minutes before declaring stale.
pub const EXNESS_MAX_AGE_SECS: i64 = 180;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExnessBar {
    /// Unix timestamp in seconds (MT5 server time, UTC).
    pub t: i64,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    #[serde(default)]
    pub v: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExnessTick {
    #[serde(default)]
    pub bid: f64,
    #[serde(default)]
    pub ask: f64,
    /// Unix timestamp in seconds (MT5 server time, UTC).
    #[serde(default)]
    pub time: i64,
}

impl ExnessTick {
    /// Mid price when both sides are valid and not crossed; otherwise the valid
    /// bid (or ask). Returns `None` when neither side is usable.
    pub fn mid(&self) -> Option<f64> {
        let bid_ok = self.bid.is_finite() && self.bid > 0.0;
        let ask_ok = self.ask.is_finite() && self.ask > 0.0;
        if bid_ok && ask_ok && self.ask >= self.bid {
            Some((self.bid + self.ask) / 2.0)
        } else if bid_ok {
            Some(self.bid)
        } else if ask_ok {
            Some(self.ask)
        } else {
            None
        }
    }

    /// Real bid/ask spread in USD/oz. `None` for zero, crossed or non-finite
    /// quotes; callers should treat that as "spread unavailable", not zero.
    pub fn spread(&self) -> Option<f64> {
        if self.bid.is_finite() && self.ask.is_finite() && self.bid > 0.0 && self.ask >= self.bid {
            Some(self.ask - self.bid)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExnessFile {
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub tick: Option<ExnessTick>,
    #[serde(default)]
    pub timeframes: HashMap<String, Vec<ExnessBar>>,
}

impl ExnessFile {
    pub fn updated_at_utc(&self) -> Option<DateTime<Utc>> {
        NaiveDateTime::parse_from_str(self.updated_at.trim(), "%Y.%m.%d %H:%M:%S")
            .ok()
            .map(|naive| Utc.from_utc_datetime(&naive))
    }

    pub fn age_secs(&self, now: DateTime<Utc>) -> Option<i64> {
        self.updated_at_utc()
            .map(|updated| (now - updated).num_seconds().max(0))
    }

    pub fn is_fresh(&self, now: DateTime<Utc>, max_age_secs: i64) -> bool {
        self.age_secs(now)
            .map(|age| age <= max_age_secs.max(1))
            .unwrap_or(false)
    }
}

/// Parse `data/exness_bars.json`. Missing/corrupt files return `None`.
pub fn load_exness_file(path: impl AsRef<Path>) -> Option<ExnessFile> {
    let path = path.as_ref();
    let content = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<ExnessFile>(&content) {
        Ok(file) => Some(file),
        Err(err) => {
            warn!(
                "⚠️ [ExnessFeed] File {} không hợp lệ ({}).",
                path.display(),
                err
            );
            None
        }
    }
}

fn valid_bar(bar: &ExnessBar) -> bool {
    bar.t > 0
        && bar.o.is_finite()
        && bar.h.is_finite()
        && bar.l.is_finite()
        && bar.c.is_finite()
        && bar.o > 0.0
        && bar.h > 0.0
        && bar.l > 0.0
        && bar.c > 0.0
        && bar.h >= bar.l
        && bar.v.is_finite()
        && bar.v >= 0.0
}

/// Convert Exness bars to closed `Candle`s, oldest first. The EA already
/// excludes the forming bar, so no additional repaint filtering is needed.
pub fn exness_candles(
    file: &ExnessFile,
    label: &str,
    timeframe: Timeframe,
    limit: usize,
) -> Vec<Candle> {
    let Some(bars) = file.timeframes.get(label) else {
        return Vec::new();
    };
    let mut candles: Vec<Candle> = bars
        .iter()
        .filter(|bar| valid_bar(bar))
        .map(|bar| Candle {
            timestamp: bar.t.saturating_mul(1000),
            open: bar.o,
            high: bar.h,
            low: bar.l,
            close: bar.c,
            volume: bar.v,
            taker_buy_volume: 0.0,
            timeframe,
        })
        .collect();
    candles.sort_by_key(|candle| candle.timestamp);
    candles.dedup_by_key(|candle| candle.timestamp);
    if candles.len() > limit.max(1) {
        let start = candles.len() - limit.max(1);
        candles.drain(0..start);
    }
    candles
}

pub fn timeframe_label(timeframe: Timeframe) -> &'static str {
    match timeframe {
        Timeframe::M15 => "M15",
        Timeframe::H1 => "H1",
        Timeframe::H4 => "H4",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn file(json: &str) -> ExnessFile {
        serde_json::from_str(json).expect("valid test json")
    }

    #[test]
    fn parses_fresh_file_and_candles() {
        let json = r#"{
            "symbol":"XAUUSDm","server":"Exness-MT5Trial7",
            "updated_at":"2026.11.17 17:52:38",
            "tick":{"bid":4112.39,"ask":4112.63,"time":1794936758},
            "timeframes":{
                "M15":[
                    {"t":1794296700,"o":4114.96,"h":4116.52,"l":4112.13,"c":4112.39,"v":2200},
                    {"t":1794297600,"o":4112.45,"h":4118.68,"l":4112.17,"c":4118.21,"v":2907}
                ],
                "H1":[{"t":1794296400,"o":4110.0,"h":4120.0,"l":4100.0,"c":4115.0,"v":10000}]
            }
        }"#;
        let parsed = file(json);
        assert_eq!(parsed.symbol, "XAUUSDm");
        assert_eq!(parsed.server, "Exness-MT5Trial7");
        assert_eq!(
            parsed.tick.as_ref().and_then(ExnessTick::mid),
            Some(4112.51)
        );
        let now = Utc.with_ymd_and_hms(2026, 11, 17, 17, 55, 0).unwrap();
        assert!(parsed.is_fresh(now, EXNESS_MAX_AGE_SECS));
        assert_eq!(parsed.age_secs(now), Some(142));
        let candles = exness_candles(&parsed, "M15", Timeframe::M15, 10);
        assert_eq!(candles.len(), 2);
        assert_eq!(candles[0].timestamp, 1794296700000);
        assert_eq!(candles[1].close, 4118.21);
        assert_eq!(candles[1].volume, 2907.0);
        assert_eq!(candles[1].taker_buy_volume, 0.0);
    }

    #[test]
    fn rejects_stale_and_corrupt_files() {
        let parsed = file(r#"{"updated_at":"2026.11.17 17:00:00","timeframes":{}}"#);
        let now = Utc.with_ymd_and_hms(2026, 11, 17, 17, 10, 0).unwrap();
        assert!(!parsed.is_fresh(now, EXNESS_MAX_AGE_SECS));
        assert_eq!(parsed.age_secs(now), Some(600));
        assert!(load_exness_file("/tmp/does-not-exist-exness.json").is_none());
    }

    #[test]
    fn tick_mid_handles_missing_and_crossed_quotes() {
        let tick = ExnessTick {
            bid: 4100.0,
            ask: 4100.4,
            time: 1,
        };
        assert_eq!(tick.mid(), Some(4100.2));
        assert!((tick.spread().expect("spread") - 0.4).abs() < 1e-9);
        let bid_only = ExnessTick {
            bid: 4100.0,
            ask: 0.0,
            time: 1,
        };
        assert_eq!(bid_only.mid(), Some(4100.0));
        assert_eq!(bid_only.spread(), None);
        let crossed = ExnessTick {
            bid: 4101.0,
            ask: 4100.0,
            time: 1,
        };
        assert_eq!(crossed.mid(), Some(4101.0));
        assert_eq!(crossed.spread(), None);
        let empty = ExnessTick::default();
        assert_eq!(empty.mid(), None);
        assert_eq!(empty.spread(), None);
    }

    #[test]
    fn filters_invalid_bars_and_applies_limit() {
        let parsed = file(
            r#"{"timeframes":{"H1":[
            {"t":1,"o":0,"h":1,"l":1,"c":1,"v":1},
            {"t":2,"o":100,"h":101,"l":99,"c":100,"v":5},
            {"t":3,"o":101,"h":102,"l":100,"c":101,"v":6}
        ]}}"#,
        );
        let candles = exness_candles(&parsed, "H1", Timeframe::H1, 1);
        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].timestamp, 3000);
        assert_eq!(candles[0].close, 101.0);
    }
}
