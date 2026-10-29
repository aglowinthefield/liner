pub mod cache;
pub mod config;
pub mod db;
pub mod health;
pub mod metrics;

use axum::{routing::get, Router};
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
    paths(health::healthz),
    components(schemas(health::Health)),
    info(title = "liner", version = "0.1.0", description = "Music metadata server"),
)]
pub struct ApiDoc;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/metrics", get(metrics::metrics_handler))
        .layer(metrics::layer())
        .with_state(state)
}
