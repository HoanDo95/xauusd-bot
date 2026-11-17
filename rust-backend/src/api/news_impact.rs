use super::AppState;
use crate::news_impact::summarize;
use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Default, Deserialize)]
pub(super) struct NewsImpactQuery {
    #[serde(default)]
    limit: Option<usize>,
}

/// GET /api/news-impact?limit=50
/// Read-only news impact/reaction records. Never affects signals.
pub(super) async fn get_news_impact(
    State(state): State<AppState>,
    Query(query): Query<NewsImpactQuery>,
) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let mut records = state.news_impact.read().await.clone();
    records.sort_by_key(|record| std::cmp::Reverse(record.scheduled_time));
    let updated_at = records
        .iter()
        .map(|record| record.updated_at)
        .max()
        .map(|time| time.to_rfc3339());
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let summary = summarize(&records);
    records.truncate(limit);

    Json(json!({
        "records": records,
        "summary": summary,
        "updatedAt": updated_at,
        "enabled": config.news_reaction_enabled,
        "aiEnabled": config.news_ai_analyst_enabled,
        "aiDailyCap": config.news_ai_daily_cap,
        "horizonMinutes": config.news_reaction_horizon_minutes,
        "advisoryOnly": true,
    }))
}

/// GET /api/news-impact/summary
pub(super) async fn get_news_impact_summary(State(state): State<AppState>) -> impl IntoResponse {
    let config = state.config.read().await.clone();
    let records = state.news_impact.read().await.clone();
    let summary = summarize(&records);
    Json(json!({
        "summary": summary,
        "enabled": config.news_reaction_enabled,
        "aiEnabled": config.news_ai_analyst_enabled,
        "aiDailyCap": config.news_ai_daily_cap,
        "horizonMinutes": config.news_reaction_horizon_minutes,
        "advisoryOnly": true,
    }))
}
