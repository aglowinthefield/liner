//! Needs Redis and Postgres (`docker compose up -d`).

use axum::{body::Body, http::Request, Router};
use liner::{build_router, cache::Cache, ratelimit::RateLimitConfig, AppState};
use tower::ServiceExt;

async fn app(anon: u32) -> Router {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into());
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://liner:liner@localhost:5433/liner".into());
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    build_router(AppState {
        pool,
        cache: Cache::connect(&redis_url).await.unwrap(),
        mb_api_url: String::new(),
        wiki: Default::default(),
        rate: RateLimitConfig { anon_per_min: anon, key_per_min: 1000, trust_proxy: true },
    })
}

fn get(path: &str, ip: &str, key: Option<&str>) -> Request<Body> {
    let mut b = Request::get(path).header("cf-connecting-ip", ip);
    if let Some(k) = key {
        b = b.header("x-liner-key", k);
    }
    b.body(Body::empty()).unwrap()
}

fn unique_ip() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[tokio::test]
async fn anonymous_client_gets_429_after_limit() {
    let app = app(2).await;
    let ip = unique_ip();
    // /v1/match with empty params returns 400, which still counts as a request.
    for _ in 0..2 {
        let res = app.clone().oneshot(get("/v1/match?artist=&album=", &ip, None)).await.unwrap();
        assert_eq!(res.status(), 400);
    }
    let res = app.clone().oneshot(get("/v1/match?artist=&album=", &ip, None)).await.unwrap();
    assert_eq!(res.status(), 429);
    assert!(res.headers().contains_key("retry-after"));

    let other = app.oneshot(get("/v1/match?artist=&album=", &unique_ip(), None)).await.unwrap();
    assert_eq!(other.status(), 400);
}

#[tokio::test]
async fn healthz_is_not_limited() {
    let app = app(1).await;
    let ip = unique_ip();
    for _ in 0..5 {
        let res = app.clone().oneshot(get("/healthz", &ip, None)).await.unwrap();
        assert_eq!(res.status(), 200);
    }
}

#[tokio::test]
async fn unknown_api_key_is_rejected() {
    let app = app(1000).await;
    let res = app.oneshot(get("/v1/match?artist=&album=", &unique_ip(), Some("lk_nope"))).await.unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn valid_api_key_bypasses_anonymous_limit() {
    let app = app(1).await;
    let key = format!("lk_{}", uuid::Uuid::new_v4().simple());
    let pool = sqlx::PgPool::connect("postgres://liner:liner@localhost:5433/liner").await.unwrap();
    sqlx::query("INSERT INTO api_keys (name, key_hash) VALUES ('test', $1)")
        .bind(liner::ratelimit::hash_key(&key))
        .execute(&pool)
        .await
        .unwrap();
    let ip = unique_ip();
    for _ in 0..5 {
        let res = app.clone().oneshot(get("/v1/match?artist=&album=", &ip, Some(&key))).await.unwrap();
        assert_eq!(res.status(), 400);
    }
}
