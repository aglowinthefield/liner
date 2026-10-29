use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{musicbrainz, AppState};

const MAX_BATCH: usize = 100;

#[derive(Debug, Deserialize, ToSchema)]
pub struct EntityRef {
    /// MusicBrainz type with hyphens, for example `artist` or `release-group`.
    pub entity_type: String,
    pub mbid: Uuid,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct NamesRequest {
    pub entities: Vec<EntityRef>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EntityName {
    pub entity_type: String,
    pub mbid: Uuid,
    /// `null` when the lookup failed.
    pub name: Option<String>,
    pub artist_name: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NamesResponse {
    pub entities: Vec<EntityName>,
}

#[utoipa::path(
    post, path = "/v1/entities/names",
    request_body = NamesRequest,
    responses(
        (status = 200, description = "Names in request order", body = NamesResponse),
        (status = 400, description = "Too many entities"),
    ),
    tag = "entities"
)]
pub async fn names(
    State(state): State<AppState>,
    Json(body): Json<NamesRequest>,
) -> impl IntoResponse {
    if body.entities.len() > MAX_BATCH {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("at most {MAX_BATCH} entities per request") }),
            ),
        )
            .into_response();
    }
    let entities = stream::iter(body.entities)
        .map(|e| {
            let url = state.mb_api_url.clone();
            async move {
                let info = musicbrainz::lookup_entity(&url, &e.entity_type, e.mbid).await;
                EntityName {
                    entity_type: e.entity_type,
                    mbid: e.mbid,
                    name: info.as_ref().map(|i| i.name.clone()),
                    artist_name: info.map(|i| i.artist_name),
                }
            }
        })
        .buffered(10)
        .collect::<Vec<_>>()
        .await;
    (StatusCode::OK, Json(NamesResponse { entities })).into_response()
}
