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
        (status = 200, description = "Release group details", body = super::model::ReleaseGroupResponse),
        (status = 404, description = "No such release group in MusicBrainz"),
        (status = 500, description = "Failed to fetch release group"),
    ),
    tag = "release-group"
)]
pub async fn get_release_group(
    State(state): State<AppState>,
    Path(mbid): Path<Uuid>,
) -> impl IntoResponse {
    match model::get_release_group(&state.mb_api_url, mbid, &state.cache).await {
        Ok(release_group) => {
            let cover = state.cover_art.front(&state.cache, mbid).await;
            (
                StatusCode::OK,
                Json(model::ReleaseGroupResponse {
                    release_group,
                    cover,
                }),
            )
                .into_response()
        }
        Err(e) => crate::musicbrainz::error_response(&e, "release group"),
    }
}
