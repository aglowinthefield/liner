//! Drives the router against a fake MusicBrainz server. Needs Redis (`docker compose up -d redis`).

use axum::{body::Body, extract::Query, http::Request, routing::get, Json, Router};
use http_body_util::BodyExt;
use liner::{build_router, cache::Cache, AppState};
use serde_json::{json, Value};
use std::collections::HashMap;
use tower::ServiceExt;

const ARTIST: &str = "11111111-1111-1111-1111-111111111111";
const RG: &str = "22222222-2222-2222-2222-222222222222";

async fn fake_mb() -> String {
    let app = Router::new()
        .route(
            "/ws/2/artist/{id}",
            get(|| async {
                Json(json!({
                    "id": ARTIST, "name": "Fake Band", "sort-name": "Fake Band",
                    "type": "Group", "country": "US",
                    "life-span": {"begin": "1990", "end": null},
                    "relations": [{"type": "official homepage", "url": {"resource": "https://fake.example"}}]
                }))
            }),
        )
        .route(
            "/ws/2/release-group/{id}",
            get(|| async {
                Json(json!({
                    "id": RG, "title": "Fake Album",
                    "artist-credit": [{"name": "Fake Band", "joinphrase": ""}]
                }))
            }),
        )
        .route(
            "/ws/2/release-group",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                if q.contains_key("query") {
                    Json(json!({"release-groups": [{
                        "id": RG, "title": "Fake Album", "score": 100, "primary-type": "Album",
                        "first-release-date": "1999-01-01",
                        "artist-credit": [{"name": "Fake Band", "joinphrase": "", "artist": {"id": ARTIST}}]
                    }]}))
                } else {
                    Json(json!({"release-groups": [
                        {"id": RG, "title": "Old", "first-release-date": "1995"},
                        {"id": RG, "title": "New", "first-release-date": "2005"}
                    ]}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn router() -> Router {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into());
    let cache = Cache::connect(&redis_url).await.unwrap();
    build_router(AppState {
        pool: sqlx::PgPool::connect_lazy("postgres://liner:liner@localhost:5433/liner").unwrap(),
        cache,
        mb_api_url: fake_mb().await,
        wiki: Default::default(),
        lastfm: None,
        fanart: None,
        listenbrainz: Default::default(),
        cover_art: Default::default(),
        llm: None,
        rate: liner::ratelimit::RateLimitConfig {
            anon_per_min: 1000,
            key_per_min: 1000,
            trust_proxy: true,
        },
    })
}

async fn call(app: Router, req: Request<Body>) -> (u16, Value) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn artist_sorts_release_groups_newest_first() {
    let (status, body) = call(
        router().await,
        Request::get(format!("/v1/artist/{ARTIST}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["name"], "Fake Band");
    assert_eq!(body["begin_date"], "1990");
    assert_eq!(body["links"][0]["url"], "https://fake.example");
    assert_eq!(body["release_groups"][0]["title"], "New");
}

#[tokio::test]
async fn match_is_confident_on_exact_title() {
    let uri = "/v1/match?artist=Fake%20Band&album=Fake%20Album";
    let (status, body) = call(
        router().await,
        Request::get(uri).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "confident");
    assert_eq!(body["candidates"][0]["mbid"], RG);
    assert_eq!(body["candidates"][0]["artist_mbid"], ARTIST);
    assert_eq!(body["artist_mbid"], ARTIST);
    assert_eq!(body["release_group_mbid"], RG);
}

#[tokio::test]
async fn match_rejects_empty_album() {
    let (status, _) = call(
        router().await,
        Request::get("/v1/match?artist=x&album=")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn entity_names_resolves_release_group() {
    let req = Request::post("/v1/entities/names")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"entities": [{"entity_type": "release-group", "mbid": RG}]}).to_string(),
        ))
        .unwrap();
    let (status, body) = call(router().await, req).await;
    assert_eq!(status, 200);
    assert_eq!(body["entities"][0]["name"], "Fake Album");
    assert_eq!(body["entities"][0]["artist_name"], "Fake Band");
}
