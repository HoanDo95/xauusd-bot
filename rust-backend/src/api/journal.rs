//! Read-only trigger-journal API.
//!
//! The journal is an analysis log, not live trade state. These endpoints never
//! send Telegram, insert a signal or change configuration.

use crate::api::AppState;
use crate::trigger_journal::{TriggerEvent, TriggerSummary};
use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct JournalQuery {
    pub limit: Option<usize>,
}

pub async fn get_trigger_journal(
    State(state): State<AppState>,
    Query(query): Query<JournalQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(200).clamp(1, 2000);
    Json::<Vec<TriggerEvent>>(state.trigger_journal.load(Some(limit)))
}

pub async fn get_trigger_journal_summary(State(state): State<AppState>) -> impl IntoResponse {
    Json::<TriggerSummary>(state.trigger_journal.summary())
}
