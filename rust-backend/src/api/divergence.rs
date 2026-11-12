use super::AppState;
use crate::models::BotConfig;
use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;

/// Enabled timeframes in dashboard order.
fn enabled_timeframes(config: &BotConfig) -> Vec<&'static str> {
    let mut timeframes = Vec::new();
    if config.divergence_alert_m15 {
        timeframes.push("M15");
    }
    if config.divergence_alert_h1 {
        timeframes.push("H1");
    }
    if config.divergence_alert_h4 {
        timeframes.push("H4");
    }
    timeframes
}

/// GET /api/divergence-alerts — standalone RSI divergence warnings.
///
/// Additive/read-only: alerts are advisory momentum notes, never trade signals.
pub(super) async fn get_divergence_alerts(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let alerts = state.divergence_alerts.read().await.clone();

    Json(json!({
        "enabled": config.divergence_alert_enabled,
        "timeframes": enabled_timeframes(&config),
        "alerts": alerts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_timeframes_follow_config_flags() {
        let all = BotConfig::default();
        assert_eq!(enabled_timeframes(&all), vec!["M15", "H1", "H4"]);

        let only_h4 = BotConfig {
            divergence_alert_m15: false,
            divergence_alert_h1: false,
            ..BotConfig::default()
        };
        assert_eq!(enabled_timeframes(&only_h4), vec!["H4"]);

        let none = BotConfig {
            divergence_alert_m15: false,
            divergence_alert_h1: false,
            divergence_alert_h4: false,
            ..BotConfig::default()
        };
        assert!(enabled_timeframes(&none).is_empty());
    }
}
