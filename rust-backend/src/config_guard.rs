//! Frozen forward-test profile guard.
//!
//! Config drift (for example `timeframe=H1` or confirmation disabled) must be
//! visible immediately instead of silently changing live behavior. The guard
//! exposes errors/warnings through `/api/health` and logs them on startup and
//! on every config update. It never mutates the config.

use crate::models::{BotConfig, Timeframe};
use serde::Serialize;

pub const EXPECTED_ACCOUNT_BALANCE: f64 = 10_000.0;
pub const EXPECTED_RISK_PERCENTAGE: f64 = 0.3;
pub const EXPECTED_MAX_DAILY_RISK_PERCENT: f64 = 1.0;
pub const EXPECTED_MAX_DAILY_LOSS_R: f64 = 3.0;
pub const EXPECTED_AI_MIN_CONFIDENCE: f64 = 0.65;
pub const EXPECTED_MAX_SIGNALS_PER_DAY: usize = 3;
pub const EXPECTED_MAX_CONCURRENT_SIGNALS: usize = 2;
pub const EXPECTED_SAME_DIRECTION_COOLDOWN_MINUTES: i64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigWarning {
    pub code: &'static str,
    /// `error` means the live strategy contract is broken; `warning` means the
    /// frozen forward-test profile changed but the engine can still run.
    pub severity: &'static str,
    pub message: String,
}

impl ConfigWarning {
    fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: "error",
            message: message.into(),
        }
    }

    fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: "warning",
            message: message.into(),
        }
    }
}

fn changed(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() > 1e-9
}

/// Evaluate the frozen profile. Errors are hard contract violations; warnings
/// are intentional-change reminders.
pub fn evaluate(config: &BotConfig) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();

    if config.timeframe != Timeframe::M15 {
        warnings.push(ConfigWarning::error(
            "timeframe_not_m15",
            format!(
                "timeframe={:?}; live entry must stay M15 (H1 is confirmation/context only).",
                config.timeframe
            ),
        ));
    }
    if !config.trigger_confirmation_enabled {
        warnings.push(ConfigWarning::error(
            "confirmation_disabled",
            "trigger_confirmation_enabled=false; the frozen live profile requires confirmation.",
        ));
    }
    if !config.risk_guardrails_enabled {
        warnings.push(ConfigWarning::error(
            "risk_guardrails_disabled",
            "risk_guardrails_enabled=false; portfolio risk limits are not enforced.",
        ));
    }

    if !config.session_filter_enabled {
        warnings.push(ConfigWarning::warning(
            "session_filter_disabled",
            "session_filter_enabled=false; live split session (07:00–20:00 + 22:00–05:00 VN) is disabled.",
        ));
    }
    if !config.daily_quota_enabled {
        warnings.push(ConfigWarning::warning(
            "quota_disabled",
            "daily_quota_enabled=false; the 3-fills/day cap is not enforced.",
        ));
    } else if config.max_signals_per_day != EXPECTED_MAX_SIGNALS_PER_DAY {
        warnings.push(ConfigWarning::warning(
            "quota_not_3",
            format!(
                "max_signals_per_day={}; expected {}.",
                config.max_signals_per_day, EXPECTED_MAX_SIGNALS_PER_DAY
            ),
        ));
    }
    if !config.session_shadow_enabled {
        warnings.push(ConfigWarning::warning(
            "session_shadow_disabled",
            "session_shadow_enabled=false; session forward sampling stopped.",
        ));
    }
    if !config.session_shadow_telegram_enabled {
        warnings.push(ConfigWarning::warning(
            "session_shadow_telegram_disabled",
            "session_shadow_telegram_enabled=false; research alerts stopped.",
        ));
    }
    if !config.htf_h4_filter_enabled || !config.htf_d1_filter_enabled {
        warnings.push(ConfigWarning::warning(
            "htf_filter_disabled",
            "HTF H4/D1 filter is not fully enabled; counter-trend signals may pass.",
        ));
    }
    if changed(
        config.ai_signal_review_min_confidence,
        EXPECTED_AI_MIN_CONFIDENCE,
    ) {
        warnings.push(ConfigWarning::warning(
            "ai_threshold_changed",
            format!(
                "ai_signal_review_min_confidence={:.2}; frozen profile is {:.2}.",
                config.ai_signal_review_min_confidence, EXPECTED_AI_MIN_CONFIDENCE
            ),
        ));
    }
    if changed(config.account_balance, EXPECTED_ACCOUNT_BALANCE) {
        warnings.push(ConfigWarning::warning(
            "account_balance_changed",
            format!(
                "account_balance={:.2}; frozen profile is {:.2}.",
                config.account_balance, EXPECTED_ACCOUNT_BALANCE
            ),
        ));
    }
    if changed(config.risk_percentage, EXPECTED_RISK_PERCENTAGE) {
        warnings.push(ConfigWarning::warning(
            "risk_percentage_changed",
            format!(
                "risk_percentage={:.2}; frozen profile is {:.2}.",
                config.risk_percentage, EXPECTED_RISK_PERCENTAGE
            ),
        ));
    }
    if changed(
        config.max_daily_risk_percent,
        EXPECTED_MAX_DAILY_RISK_PERCENT,
    ) {
        warnings.push(ConfigWarning::warning(
            "daily_risk_changed",
            format!(
                "max_daily_risk_percent={:.2}; frozen profile is {:.2}.",
                config.max_daily_risk_percent, EXPECTED_MAX_DAILY_RISK_PERCENT
            ),
        ));
    }
    if changed(config.max_daily_loss_r, EXPECTED_MAX_DAILY_LOSS_R) {
        warnings.push(ConfigWarning::warning(
            "daily_loss_changed",
            format!(
                "max_daily_loss_r={:.2}; frozen profile is {:.2}.",
                config.max_daily_loss_r, EXPECTED_MAX_DAILY_LOSS_R
            ),
        ));
    }
    if config.max_concurrent_signals != EXPECTED_MAX_CONCURRENT_SIGNALS {
        warnings.push(ConfigWarning::warning(
            "max_concurrent_changed",
            format!(
                "max_concurrent_signals={}; frozen profile is {}.",
                config.max_concurrent_signals, EXPECTED_MAX_CONCURRENT_SIGNALS
            ),
        ));
    }
    if config.same_direction_cooldown_minutes != EXPECTED_SAME_DIRECTION_COOLDOWN_MINUTES {
        warnings.push(ConfigWarning::warning(
            "cooldown_changed",
            format!(
                "same_direction_cooldown_minutes={}; frozen profile is {}.",
                config.same_direction_cooldown_minutes, EXPECTED_SAME_DIRECTION_COOLDOWN_MINUTES
            ),
        ));
    }

    warnings
}

pub fn has_errors(warnings: &[ConfigWarning]) -> bool {
    warnings.iter().any(|warning| warning.severity == "error")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frozen_profile() -> BotConfig {
        BotConfig {
            account_balance: EXPECTED_ACCOUNT_BALANCE,
            risk_percentage: EXPECTED_RISK_PERCENTAGE,
            max_daily_risk_percent: EXPECTED_MAX_DAILY_RISK_PERCENT,
            max_daily_loss_r: EXPECTED_MAX_DAILY_LOSS_R,
            ai_signal_review_min_confidence: EXPECTED_AI_MIN_CONFIDENCE,
            max_signals_per_day: EXPECTED_MAX_SIGNALS_PER_DAY,
            max_concurrent_signals: EXPECTED_MAX_CONCURRENT_SIGNALS,
            same_direction_cooldown_minutes: EXPECTED_SAME_DIRECTION_COOLDOWN_MINUTES,
            daily_quota_enabled: true,
            session_filter_enabled: true,
            trigger_confirmation_enabled: true,
            risk_guardrails_enabled: true,
            htf_h4_filter_enabled: true,
            htf_d1_filter_enabled: true,
            session_shadow_enabled: true,
            session_shadow_telegram_enabled: true,
            ..BotConfig::default()
        }
    }

    #[test]
    fn frozen_profile_has_no_warnings() {
        let warnings = evaluate(&frozen_profile());
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert!(!has_errors(&warnings));
    }

    #[test]
    fn h1_timeframe_is_an_error() {
        let mut config = frozen_profile();
        config.timeframe = Timeframe::H1;
        let warnings = evaluate(&config);
        assert!(warnings.iter().any(|w| w.code == "timeframe_not_m15"));
        assert!(has_errors(&warnings));
    }

    #[test]
    fn confirmation_off_is_an_error() {
        let mut config = frozen_profile();
        config.trigger_confirmation_enabled = false;
        let warnings = evaluate(&config);
        assert!(warnings.iter().any(|w| w.code == "confirmation_disabled"));
        assert!(has_errors(&warnings));
    }

    #[test]
    fn shadow_off_is_a_warning_not_an_error() {
        let mut config = frozen_profile();
        config.session_shadow_enabled = false;
        let warnings = evaluate(&config);
        assert!(warnings.iter().any(|w| w.code == "session_shadow_disabled"));
        assert!(!has_errors(&warnings));
    }
}
