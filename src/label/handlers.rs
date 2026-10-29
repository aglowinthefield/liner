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
    get, path = "/v1/label/{mbid}",
    params(("mbid" = Uuid, Path, description = "MusicBrainz label ID")),
    responses(
        (status = 200, description = "Label details", body = super::model::LabelDetail),
        (status = 404, description = "No such label in MusicBrainz"),
        (status = 500, description = "Failed to fetch label"),
    ),
    tag = "label"
)]
pub async fn get_label(
    State(state): State<AppState>,
    Path(mbid): Path<Uuid>,
) -> impl IntoResponse {
    match model::get_label(&state.mb_api_url, mbid, &state.cache).await {
        Ok(label) => (StatusCode::OK, Json(serde_json::json!(label))).into_response(),
        Err(e) => crate::musicbrainz::error_response(&e, "label"),
    }
}
