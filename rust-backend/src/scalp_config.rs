//! Separate filter config for the scalp service.
//!
//! Kept out of `BotConfig` so changing scalp filters does not alter the swing
//! service config hash or forward-test baseline.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const DEFAULT_PATH: &str = "data/scalp_filters.json";

fn default_true() -> bool {
    true
}

fn default_min_confidence() -> u8 {
    60
}

fn default_max_alerts_per_day() -> usize {
    3
}

fn default_cooldown_minutes() -> i64 {
    45
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScalpFilterConfig {
    /// Enable H4/D1 HTF filter for scalp entries.
    #[serde(default = "default_true")]
    pub htf_filter_enabled: bool,
    /// Enable BUY RSI H1 oversold guard.
    #[serde(default = "default_true")]
    pub rsi_h1_guard_enabled: bool,
    /// Minimum PRIMARY confidence.
    #[serde(default = "default_min_confidence")]
    pub min_confidence: u8,
    /// Maximum research alerts per VN day.
    #[serde(default = "default_max_alerts_per_day")]
    pub max_alerts_per_day: usize,
    /// Cooldown between alerts, in minutes.
    #[serde(default = "default_cooldown_minutes")]
    pub cooldown_minutes: i64,
}

impl Default for ScalpFilterConfig {
    fn default() -> Self {
        Self {
            htf_filter_enabled: true,
            rsi_h1_guard_enabled: true,
            min_confidence: default_min_confidence(),
            max_alerts_per_day: default_max_alerts_per_day(),
            cooldown_minutes: default_cooldown_minutes(),
        }
    }
}

impl ScalpFilterConfig {
    pub fn sanitized(mut self) -> Self {
        self.min_confidence = self.min_confidence.min(100);
        self.max_alerts_per_day = self.max_alerts_per_day.clamp(1, 10);
        self.cooldown_minutes = self.cooldown_minutes.clamp(5, 240);
        self
    }
}

pub struct ScalpFilterStore {
    path: PathBuf,
}

impl ScalpFilterStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_env() -> Self {
        let path = std::env::var("SCALP_FILTERS_PATH").unwrap_or_else(|_| DEFAULT_PATH.into());
        Self::new(path)
    }

    pub fn load(&self) -> ScalpFilterConfig {
        let Ok(content) = fs::read_to_string(&self.path) else {
            return ScalpFilterConfig::default();
        };
        serde_json::from_str::<ScalpFilterConfig>(&content)
            .map(ScalpFilterConfig::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self, config: &ScalpFilterConfig) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let temp = self.path.with_extension("json.tmp");
        let sanitized = config.clone().sanitized();
        let payload = serde_json::to_string_pretty(&sanitized).map_err(|err| err.to_string())?;
        fs::write(&temp, payload).map_err(|err| err.to_string())?;
        fs::rename(&temp, &self.path).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let config = ScalpFilterConfig::default();
        assert!(config.htf_filter_enabled);
        assert!(config.rsi_h1_guard_enabled);
        assert_eq!(config.min_confidence, 60);
        assert_eq!(config.max_alerts_per_day, 3);
        assert_eq!(config.cooldown_minutes, 45);
    }

    #[test]
    fn sanitize_clamps_values() {
        let config = ScalpFilterConfig {
            min_confidence: 120,
            max_alerts_per_day: 50,
            cooldown_minutes: 1,
            ..ScalpFilterConfig::default()
        }
        .sanitized();
        assert_eq!(config.min_confidence, 100);
        assert_eq!(config.max_alerts_per_day, 10);
        assert_eq!(config.cooldown_minutes, 5);
    }
}
