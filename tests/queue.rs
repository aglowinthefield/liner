//! The job queue's handling of rate limits, retries, and scrape metrics.
//! One test, because it clears the shared queue and the cooldown flag in Redis.
//! Needs Postgres and Redis (`docker compose up -d`).

use axum::{
    extract::Path,
    http::{header, StatusCode},
    routing::get,
    Json, Router,
};
use liner::{
    cache::Cache, jobs, ratelimit::RateLimitConfig, sources::wikipedia::Wikipedia, worker, AppState,
};
use redis::AsyncCommands;
use serde_json::json;
use uuid::Uuid;

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn fake_mb() -> String {
    serve(Router::new().route("/ws/2/release-group", get(|| async { Json(json!({"release-groups": []})) })).route(
        "/ws/2/artist/{id}",
        get(|Path(id): Path<String>| async move {
            Json(json!({"id": id, "name": "Fake Band", "sort-name": "Fake Band",
                "relations": [{"type": "wikidata", "url": {"resource": "https://www.wikidata.org/wiki/Q42"}}]}))
        }),
    ))
    .await
}

/// Wikidata answers 429 with `Retry-After: 120`.
async fn fake_limited_wikidata() -> String {
    serve(Router::new().route(
        "/w/api.php",
        get(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, "120")],
                "slow down",
            )
        }),
    ))
    .await
}

#[tokio::test]
async fn rate_limit_pauses_enrich_jobs_without_using_attempts() {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into());
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://liner:liner@localhost:5433/liner".into());
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    sqlx::query("DELETE FROM jobs")
        .execute(&pool)
        .await
        .unwrap();
    let mut redis = redis::Client::open(redis_url.as_str())
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let _: () = redis.del("liner:cooldown:enrich").await.unwrap();

    let wikidata = fake_limited_wikidata().await;
    let state = AppState {
        pool: pool.clone(),
        cache: Cache::connect(&redis_url).await.unwrap(),
        mb_api_url: fake_mb().await,
        wiki: Wikipedia::new(&wikidata, &wikidata),
        lastfm: None,
        fanart: None,
        listenbrainz: Default::default(),
        cover_art: Default::default(),
        llm: None,
        rate: RateLimitConfig {
            anon_per_min: 1000,
            key_per_min: 1000,
            trust_proxy: true,
        },
    };

    let mbid = Uuid::new_v4();
    jobs::enqueue(&pool, jobs::KIND_ENRICH, mbid).await.unwrap();
    let other = Uuid::new_v4();
    jobs::enqueue(&pool, jobs::KIND_ENRICH, other)
        .await
        .unwrap();

    // The first job hits the limit: it goes back to pending, keeps its attempt, and waits
    // out Retry-After.
    assert!(worker::run_one(&state).await.unwrap());
    let (status, attempts, wait, error): (String, i32, f64, Option<String>) = sqlx::query_as(
        "SELECT status, attempts, EXTRACT(EPOCH FROM run_after - now())::float8, last_error
         FROM jobs WHERE mbid = $1",
    )
    .bind(mbid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((status.as_str(), attempts), ("pending", 0));
    assert!((100.0..=121.0).contains(&wait), "waits {wait}s");
    assert!(error.unwrap().contains("rate limited"));

    // Every enrich job is now left alone, so the other one is untouched.
    assert!(!worker::run_one(&state).await.unwrap());
    let untouched: i32 = sqlx::query_scalar("SELECT attempts FROM jobs WHERE mbid = $1")
        .bind(other)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(untouched, 0);

    // The queue gauges read from the table.
    let stats = jobs::stats(&pool).await.unwrap();
    let pending = stats
        .iter()
        .find(|s| s.kind == "enrich" && s.status == "pending")
        .unwrap();
    assert_eq!(pending.count, 2);

    let _: () = redis.del("liner:cooldown:enrich").await.unwrap();
    sqlx::query("DELETE FROM jobs")
        .execute(&pool)
        .await
        .unwrap();
}
