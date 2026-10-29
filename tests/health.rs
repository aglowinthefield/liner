use axum::{body::Body, http::Request};
use liner::{build_router, cache::Cache, AppState};
use tower::ServiceExt;

#[tokio::test]
async fn healthz_returns_ok() {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into());
    let state = AppState {
        pool: sqlx::PgPool::connect_lazy("postgres://liner:liner@localhost:5433/liner").unwrap(),
        cache: Cache::connect(&redis_url).await.unwrap(),
        mb_api_url: String::new(),
        rate: liner::ratelimit::RateLimitConfig { anon_per_min: 1000, key_per_min: 1000, trust_proxy: true },
    };
    let res = build_router(state)
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}
