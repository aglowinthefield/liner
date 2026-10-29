use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use super::model;
use crate::AppState;

#[derive(Deserialize, IntoParams, ToSchema)]
pub struct SearchParams {
    pub q: String,
}

#[utoipa::path(
    get, path = "/v1/search",
    params(SearchParams),
    responses(
        (status = 200, description = "Search results", body = super::model::SearchResponse),
        (status = 400, description = "Empty query"),
        (status = 500, description = "Search failed"),
    ),
    tag = "search"
)]
pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    if params.q.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Query parameter 'q' must not be empty" })),
        )
            .into_response();
    }

    match model::search(&state.mb_api_url, params.q.trim(), &state.cache).await {
        Ok(response) => (StatusCode::OK, Json(serde_json::json!(response))).into_response(),
        Err(e) => {
            tracing::error!("Search failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Search failed" })),
            )
                .into_response()
        }
    }
}
