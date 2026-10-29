use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use uuid::Uuid;

use super::model::{self, ArtistResponse};
use crate::{enrich, musicbrainz, AppState};

#[utoipa::path(
    get, path = "/v1/artist/{mbid}",
    params(("mbid" = Uuid, Path, description = "MusicBrainz artist ID")),
    responses(
        (status = 200, description = "Artist details with bio and images", body = ArtistResponse),
        (status = 404, description = "No such artist in MusicBrainz"),
        (status = 500, description = "Failed to fetch artist"),
    ),
    tag = "artist"
)]
pub async fn get_artist(
    State(state): State<AppState>,
    Path(mbid): Path<Uuid>,
) -> impl IntoResponse {
    match model::get_artist(&state.mb_api_url, mbid, &state.cache).await {
        Ok(artist) => {
            let (enrichment, status) = enrich::lookup(&state, mbid).await;
            let body = ArtistResponse {
                artist,
                bio: enrichment.bio,
                images: enrichment.images,
                tags: enrichment.tags,
                enrichment: status,
            };
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => musicbrainz::error_response(&e, "artist"),
    }
}
