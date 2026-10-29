//! Artist enrichment against fake MusicBrainz, Wikidata, and Wikipedia servers.
//! Needs Postgres and Redis (`docker compose up -d`).

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::{body::Body, extract::Path, extract::State, http::{Request, StatusCode}, routing::get, Json, Router};
use http_body_util::BodyExt;
use liner::{build_router, cache::Cache, ratelimit::RateLimitConfig, sources::wikipedia::Wikipedia, AppState};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn fake_mb() -> String {
    serve(
        Router::new()
            .route(
                "/ws/2/artist/{id}",
                get(|Path(id): Path<String>| async move {
                    if id.starts_with("00000000") {
                        return (StatusCode::NOT_FOUND, Json(json!({"error": "Not Found"})));
                    }
                    let links = if id.starts_with("ffffffff") {
                        json!([])
                    } else {
                        json!([{"type": "wikidata", "url": {"resource": "https://www.wikidata.org/wiki/Q42"}}])
                    };
                    (StatusCode::OK, Json(json!({"id": id, "name": "Fake Band", "sort-name": "Fake Band", "relations": links})))
                }),
            )
            .route("/ws/2/release-group", get(|| async { Json(json!({"release-groups": []})) })),
    )
    .await
}

/// Returns the base URL and a counter of Wikipedia summary requests.
async fn fake_wiki() -> (String, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let url = serve(
        Router::new()
            .route(
                "/w/api.php",
                get(|| async {
                    Json(json!({"entities": {"Q42": {"sitelinks": {"enwiki": {"title": "Fake Band (band)"}}}}}))
                }),
            )
            .route(
                "/api/rest_v1/page/summary/{title}",
                get(|State(hits): State<Arc<AtomicUsize>>, Path(title): Path<String>| async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(title, "Fake Band (band)");
                    Json(json!({
                        "type": "standard", "title": "Fake Band (band)",
                        "extract": "Fake Band is a band.",
                        "content_urls": {"desktop": {"page": "https://en.wikipedia.org/wiki/Fake_Band_(band)"}},
                        "thumbnail": {"source": "https://img.example/t.jpg", "width": 330, "height": 220},
                        "originalimage": {"source": "https://img.example/o.jpg", "width": 3000, "height": 2000}
                    }))
                }),
            )
            .with_state(hits.clone()),
    )
    .await;
    (url, hits)
}

async fn app(wiki_url: &str) -> Router {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into());
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://liner:liner@localhost:5433/liner".into());
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    build_router(AppState {
        pool,
        cache: Cache::connect(&redis_url).await.unwrap(),
        mb_api_url: fake_mb().await,
        wiki: Wikipedia::new(wiki_url, wiki_url),
        rate: RateLimitConfig { anon_per_min: 100_000, key_per_min: 100_000, trust_proxy: true },
    })
}

async fn get_json(app: &Router, uri: &str) -> (u16, Value) {
    let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn artist_gets_bio_and_images_then_serves_from_postgres() {
    let (wiki_url, hits) = fake_wiki().await;
    let app = app(&wiki_url).await;
    let uri = format!("/v1/artist/{}", Uuid::new_v4());

    let (status, body) = get_json(&app, &uri).await;
    assert_eq!(status, 200);
    assert_eq!(body["name"], "Fake Band");
    assert_eq!(body["bio"]["summary"], "Fake Band is a band.");
    assert_eq!(body["bio"]["source"], "wikipedia");
    assert_eq!(body["bio"]["license"], "CC BY-SA 4.0");
    assert_eq!(body["bio"]["url"], "https://en.wikipedia.org/wiki/Fake_Band_(band)");
    assert_eq!(body["images"].as_array().unwrap().len(), 2);

    let (_, again) = get_json(&app, &uri).await;
    assert_eq!(again["bio"]["summary"], "Fake Band is a band.");
    assert_eq!(again["images"].as_array().unwrap().len(), 2);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "second request should not call Wikipedia");
}

#[tokio::test]
async fn artist_without_wikidata_link_has_no_bio() {
    let (wiki_url, hits) = fake_wiki().await;
    let app = app(&wiki_url).await;
    let id = format!("ffffffff-{}", &Uuid::new_v4().to_string()[9..]);
    let (status, body) = get_json(&app, &format!("/v1/artist/{id}")).await;
    assert_eq!(status, 200);
    assert!(body["bio"].is_null());
    assert_eq!(body["images"], json!([]));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn wikipedia_outage_still_serves_artist() {
    let app = app("http://127.0.0.1:1").await;
    let (status, body) = get_json(&app, &format!("/v1/artist/{}", Uuid::new_v4())).await;
    assert_eq!(status, 200);
    assert!(body["bio"].is_null());
}

#[tokio::test]
async fn unknown_artist_is_404() {
    let (wiki_url, _) = fake_wiki().await;
    let app = app(&wiki_url).await;
    let id = format!("00000000-{}", &Uuid::new_v4().to_string()[9..]);
    let (status, _) = get_json(&app, &format!("/v1/artist/{id}")).await;
    assert_eq!(status, 404);
}
