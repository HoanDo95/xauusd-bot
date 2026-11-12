use super::AppState;
use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;

/// GET /api/macro-flow — read-only macro/flow snapshot.
///
/// Returns `disabled` when the dashboard toggle is off, `loading` until the
/// worker has produced its first snapshot, and `ok` with the snapshot otherwise.
/// It never touches signals, quota, Telegram or trading.
pub(super) async fn get_macro_flow(State(state): State<AppState>) -> impl IntoResponse {
    let enabled = state.config.read().await.macro_flow_enabled;
    if !enabled {
        return Json(json!({ "status": "disabled" }));
    }

    let snapshot = state.macro_flow.read().await.clone();
    match snapshot {
        Some(snapshot) => Json(json!({ "status": "ok", "snapshot": snapshot })),
        None => Json(json!({ "status": "loading" })),
    }
}
