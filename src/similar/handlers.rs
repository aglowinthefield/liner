use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use super::model::{self, SimilarResponse, SimilarStatus};
use crate::{artist::model::get_artist, musicbrainz, AppState};

#[derive(Debug, Deserialize, IntoParams)]
pub struct SimilarParams {
    /// How many artists to return, 1 to 50. Default 20.
    pub limit: Option<usize>,
}

#[utoipa::path(
    get, path = "/v1/artist/{mbid}/similar",
    params(("mbid" = Uuid, Path, description = "MusicBrainz artist ID"), SimilarParams),
    responses(
        (status = 200, description = "Similar artists", body = SimilarResponse),
        (status = 202, description = "Nothing yet; a job is working on it. Ask again in a few seconds.", body = SimilarResponse),
        (status = 404, description = "No such artist in MusicBrainz"),
        (status = 500, description = "Failed to look up similar artists"),
    ),
    tag = "artist"
)]
pub async fn get_similar(
    State(state): State<AppState>,
    Path(mbid): Path<Uuid>,
    Query(params): Query<SimilarParams>,
) -> impl IntoResponse {
    // Look the artist up first so unknown MBIDs never start jobs.
    let artist = match get_artist(&state.mb_api_url, mbid, &state.cache).await {
        Ok(a) => a,
        Err(e) => return musicbrainz::error_response(&e, "artist"),
    };
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    match model::lookup(&state, mbid, limit).await {
        Ok((status, similar)) => {
            let code = if status == SimilarStatus::Pending {
                StatusCode::ACCEPTED
            } else {
                StatusCode::OK
            };
            (
                code,
                Json(SimilarResponse {
                    mbid,
                    name: artist.name,
                    status,
                    similar,
                }),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("similar lookup for {mbid} failed: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to look up similar artists" })),
            )
                .into_response()
        }
    }
}
