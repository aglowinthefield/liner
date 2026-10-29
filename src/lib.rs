pub mod album_match;
pub mod artist;
pub mod cache;
pub mod config;
pub mod db;
pub mod entities;
pub mod health;
pub mod label;
pub mod metrics;
pub mod musicbrainz;
pub mod release_group;
pub mod search;

use axum::{routing::{get, post}, Router};
use sqlx::PgPool;
use utoipa::OpenApi;

use crate::cache::Cache;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub cache: Cache,
    pub mb_api_url: String,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        health::healthz,
        artist::handlers::get_artist,
        label::handlers::get_label,
        release_group::handlers::get_release_group,
        search::handlers::search,
        album_match::handlers::match_album,
        album_match::handlers::match_batch,
        entities::names,
    ),
    components(schemas(
        health::Health,
        artist::model::ArtistDetail,
        artist::model::ReleaseGroupSummary,
        label::model::LabelDetail,
        label::model::LabelRelease,
        label::model::LabelReleaseArtist,
        release_group::model::ReleaseGroupDetail,
        release_group::model::ArtistCredit,
        release_group::model::ExternalLink,
        release_group::model::LabelInfo,
        release_group::model::Medium,
        release_group::model::Track,
        search::model::SearchResponse,
        search::model::ArtistResult,
        search::model::ReleaseGroupResult,
        search::handlers::SearchParams,
        album_match::model::MatchRequest,
        album_match::model::AlbumToMatch,
        album_match::model::MatchResponse,
        album_match::model::AlbumMatch,
        album_match::model::MatchStatus,
        album_match::model::MatchCandidate,
        entities::EntityRef,
        entities::NamesRequest,
        entities::NamesResponse,
        entities::EntityName,
    )),
    info(title = "liner", version = "0.1.0", description = "Music metadata server"),
)]
pub struct ApiDoc;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/v1/artist/{mbid}", get(artist::handlers::get_artist))
        .route("/v1/label/{mbid}", get(label::handlers::get_label))
        .route("/v1/release-group/{mbid}", get(release_group::handlers::get_release_group))
        .route("/v1/search", get(search::handlers::search))
        .route("/v1/match", get(album_match::handlers::match_album))
        .route("/v1/match/batch", post(album_match::handlers::match_batch))
        .route("/v1/entities/names", post(entities::names))
        .route("/metrics", get(metrics::metrics_handler))
        .layer(metrics::layer())
        .with_state(state)
}
