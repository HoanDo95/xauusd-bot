//! Scalp research service placeholder.
//!
//! The separate scalp service runs with `BOT_PROFILE=scalp` and its own data
//! directory/Telegram channel. This module will host the ICT/OB short-swing
//! strategy after research/backtest. Until then it only keeps the service alive
//! and never sends signals, consumes quota or auto-trades.

use crate::api::AppState;
use std::time::Duration;
use tracing::info;

const INTERVAL_SECS: u64 = 60;

pub fn spawn_scalp_worker(state: AppState) {
    tokio::spawn(async move {
        info!("🧪 [Scalp] Research worker started (alert-only; strategy pending).");
        let mut interval = tokio::time::interval(Duration::from_secs(INTERVAL_SECS));
        loop {
            interval.tick().await;
            let config = state.config.read().await.clone();
            if !config.is_active {
                continue;
            }
            // Research/backtest phase: no Telegram, no signal, no quota.
            // The ICT/OB strategy will be added here after pre-registration.
        }
    });
}
