use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use uuid::Uuid;

use super::model;
use crate::AppState;

#[utoipa::path(
    get, path = "/v1/release-group/{mbid}",
    params(("mbid" = Uuid, Path, description = "MusicBrainz release group ID")),
    responses(
        (status = 200, description = "Release group details", body = super::model::ReleaseGroupDetail),
        (status = 500, description = "Failed to fetch release group"),
    ),
    tag = "release-group"
)]
pub async fn get_release_group(
    State(state): State<AppState>,
    Path(mbid): Path<Uuid>,
) -> impl IntoResponse {
    match model::get_release_group(&state.mb_api_url, mbid, &state.cache).await {
        Ok(detail) => (StatusCode::OK, Json(serde_json::json!(detail))).into_response(),
        Err(e) => {
            tracing::error!("Failed to fetch release group {}: {}", mbid, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to fetch release group" })),
            )
                .into_response()
        }
    }
}
