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
    get, path = "/v1/artist/{mbid}",
    params(("mbid" = Uuid, Path, description = "MusicBrainz artist ID")),
    responses(
        (status = 200, description = "Artist details", body = super::model::ArtistDetail),
        (status = 500, description = "Failed to fetch artist"),
    ),
    tag = "artist"
)]
pub async fn get_artist(
    State(state): State<AppState>,
    Path(mbid): Path<Uuid>,
) -> impl IntoResponse {
    match model::get_artist(&state.mb_api_url, mbid, &state.cache).await {
        Ok(artist) => (StatusCode::OK, Json(serde_json::json!(artist))).into_response(),
        Err(e) => {
            tracing::error!("Failed to fetch artist {}: {}", mbid, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to fetch artist" })),
            )
                .into_response()
        }
    }
}
