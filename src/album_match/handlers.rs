use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use utoipa::IntoParams;

use super::model::{self, AlbumMatch, MatchRequest, MatchResponse};
use crate::AppState;

const MAX_BATCH: usize = 100;

#[derive(Deserialize, IntoParams)]
pub struct MatchParams {
    pub artist: String,
    pub album: String,
}

#[utoipa::path(
    get, path = "/v1/match",
    params(MatchParams),
    responses(
        (status = 200, description = "Ranked release group candidates", body = AlbumMatch),
        (status = 400, description = "Empty artist or album"),
    ),
    tag = "match"
)]
pub async fn match_album(
    State(state): State<AppState>,
    Query(params): Query<MatchParams>,
) -> impl IntoResponse {
    if params.artist.trim().is_empty() || params.album.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "'artist' and 'album' must not be empty" })),
        )
            .into_response();
    }
    let m = model::match_album(&state.mb_api_url, &state.cache, &params.artist, &params.album).await;
    (StatusCode::OK, Json(m)).into_response()
}

#[utoipa::path(
    post, path = "/v1/match/batch",
    request_body = MatchRequest,
    responses(
        (status = 200, description = "One match per requested album", body = MatchResponse),
        (status = 400, description = "Too many albums"),
    ),
    tag = "match"
)]
pub async fn match_batch(
    State(state): State<AppState>,
    Json(body): Json<MatchRequest>,
) -> impl IntoResponse {
    if body.albums.len() > MAX_BATCH {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("at most {MAX_BATCH} albums per request") })),
        )
            .into_response();
    }
    let matches = stream::iter(body.albums)
        .map(|album| {
            let mb_url = state.mb_api_url.clone();
            let cache = state.cache.clone();
            async move { model::match_album(&mb_url, &cache, &album.artist_name, &album.album_name).await }
        })
        .buffered(10)
        .collect::<Vec<_>>()
        .await;
    (StatusCode::OK, Json(MatchResponse { matches })).into_response()
}
