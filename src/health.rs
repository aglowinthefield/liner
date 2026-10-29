use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct Health {
    pub status: &'static str,
}

#[utoipa::path(get, path = "/healthz", responses((status = 200, body = Health)))]
pub async fn healthz() -> Json<Health> {
    Json(Health { status: "ok" })
}
