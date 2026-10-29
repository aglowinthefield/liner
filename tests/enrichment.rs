//! Last.fm, fanart.tv, Cover Art Archive, and the similar-artists pipeline against fake servers.
//! Needs Postgres and Redis (`docker compose up -d`).

use axum::{
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::{get, head, post},
    Json, Router,
};
use http_body_util::BodyExt;
use liner::{
    build_router,
    cache::Cache,
    llm::Llm,
    ratelimit::RateLimitConfig,
    sources::{
        coverart::CoverArt, fanart::Fanart, lastfm::Lastfm, listenbrainz::ListenBrainz,
        wikipedia::Wikipedia,
    },
    worker, AppState,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

const B: &str = "b0000000-0000-4000-8000-000000000001";
const C: &str = "c0000000-0000-4000-8000-000000000002";
const INVENTED: &str = "d0000000-0000-4000-8000-000000000003";

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
                    Json(json!({"id": id, "name": "Root Band", "sort-name": "Root Band", "type": "Group", "country": "US", "relations": []}))
                }),
            )
            .route("/ws/2/release-group", get(|| async { Json(json!({"release-groups": [{"id": B, "title": "First Record", "primary-type": "Album"}]})) })),
    )
    .await
}

async fn fake_lastfm() -> String {
    serve(Router::new().route(
        "/2.0/",
        get(|axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>| async move {
            assert_eq!(q["api_key"], "test-key");
            match q["method"].as_str() {
                "artist.getinfo" => Json(json!({"artist": {
                    "url": "https://www.last.fm/music/Root+Band",
                    "bio": {"summary": "Root Band is a band. <a href=\"https://www.last.fm/music/Root+Band\">Read more on Last.fm</a>"},
                    "tags": {"tag": [{"name": "grunge"}, {"name": "rock"}]},
                }})),
                "artist.getsimilar" => Json(json!({"similarartists": {"artist": [
                    {"name": "Band B", "mbid": B, "match": "0.9"},
                    {"name": "Band C", "mbid": C, "match": "0.4"},
                    {"name": "No Mbid", "mbid": "", "match": "0.3"},
                ]}})),
                other => panic!("unexpected last.fm method {other}"),
            }
        }),
    ))
    .await
}

async fn fake_listenbrainz() -> String {
    serve(Router::new().route(
        "/similar-artists/json",
        get(|| async {
            Json(json!([
                {"artist_mbid": B, "name": "Band B", "score": 100},
                {"artist_mbid": C, "name": "Band C", "score": 80},
            ]))
        }),
    ))
    .await
}

async fn fake_fanart() -> String {
    serve(Router::new().route(
        "/v3/music/{id}",
        get(|| async {
            Json(json!({
                "artistthumb": [{"url": "https://assets.fanart.tv/t1.jpg"}, {"url": "https://assets.fanart.tv/t2.jpg"}],
                "hdmusiclogo": [{"url": "https://assets.fanart.tv/logo.png"}],
            }))
        }),
    ))
    .await
}

/// Answers every request with picks for C and B, plus one artist that was not a candidate.
async fn fake_llm() -> String {
    serve(Router::new().route(
        "/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["model"], "test-model");
            let prompt = body["messages"][1]["content"].as_str().unwrap();
            assert!(prompt.contains("Root Band"));
            assert!(prompt.contains(B) && prompt.contains(C));
            assert!(!prompt.contains("No Mbid"));
            let picks = json!({"similar": [
                {"mbid": C, "score": 0.9, "kind": "members", "reason": "Share a drummer."},
                {"mbid": INVENTED, "score": 0.99, "kind": "sound", "reason": "Not a candidate."},
                {"mbid": B, "score": 0.5, "kind": "scene", "reason": "Same city, same years."},
            ]});
            Json(json!({"choices": [{"message": {"content": picks.to_string()}}]}))
        }),
    ))
    .await
}

async fn fake_caa() -> String {
    serve(Router::new().route(
        "/release-group/{id}/front-250",
        head(|Path(id): Path<String>| async move {
            if id.starts_with("aaaa") {
                StatusCode::TEMPORARY_REDIRECT
            } else {
                StatusCode::NOT_FOUND
            }
        }),
    ))
    .await
}

async fn state(llm_url: Option<&str>) -> AppState {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into());
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://liner:liner@localhost:5433/liner".into());
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    AppState {
        pool,
        cache: Cache::connect(&redis_url).await.unwrap(),
        mb_api_url: fake_mb().await,
        wiki: Wikipedia::new("http://127.0.0.1:1", "http://127.0.0.1:1"),
        lastfm: Some(Lastfm::new(&fake_lastfm().await, "test-key")),
        fanart: Some(Fanart::new(&fake_fanart().await, "fanart-key")),
        listenbrainz: ListenBrainz::new(&fake_listenbrainz().await),
        cover_art: CoverArt::new(&fake_caa().await),
        llm: llm_url.map(|url| Llm::new(url, "llm-key", "test-model", 1000)),
        rate: RateLimitConfig {
            anon_per_min: 100_000,
            key_per_min: 100_000,
            trust_proxy: true,
        },
    }
}

async fn get_json(app: &Router, uri: &str) -> (u16, Value) {
    let res = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Runs queued jobs until none are due.
async fn drain(state: &AppState) {
    while worker::run_one(state).await.unwrap() {}
}

#[tokio::test]
async fn artist_gets_lastfm_bio_tags_and_fanart_images() {
    let app = build_router(state(None).await);
    let (status, body) = get_json(&app, &format!("/v1/artist/{}", Uuid::new_v4())).await;
    assert_eq!(status, 200);
    assert_eq!(body["bio"]["source"], "lastfm");
    assert_eq!(body["bio"]["summary"], "Root Band is a band.");
    assert_eq!(body["bio"]["license"], "CC BY-SA 3.0");
    assert_eq!(body["tags"], json!(["grunge", "rock"]));
    let images: Vec<(&str, &str)> = body["images"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["source"].as_str().unwrap(), i["kind"].as_str().unwrap()))
        .collect();
    assert_eq!(images, vec![("fanart", "primary"), ("fanart", "logo")]);
}

#[tokio::test]
async fn cover_art_is_found_or_absent() {
    let state = state(None).await;
    let found = Uuid::parse_str("aaaa0000-0000-4000-8000-000000000000").unwrap();
    let cover = state.cover_art.front(&state.cache, found).await.unwrap();
    assert_eq!(
        cover.medium,
        format!("https://coverartarchive.org/release-group/{found}/front-500")
    );
    assert!(state
        .cover_art
        .front(
            &state.cache,
            Uuid::parse_str("bbbb0000-0000-4000-8000-000000000000").unwrap()
        )
        .await
        .is_none());
}

/// One test, because jobs are shared through the database and a worker in one scenario
/// would pick up jobs queued by another.
#[tokio::test]
async fn similar_artists_pipeline() {
    // Without an LLM the artist is served as merged candidates.
    let plain = state(None).await;
    sqlx::query("DELETE FROM jobs")
        .execute(&plain.pool)
        .await
        .unwrap();
    let app = build_router(plain.clone());
    let root = Uuid::new_v4();

    let (status, body) = get_json(&app, &format!("/v1/artist/{root}/similar")).await;
    assert_eq!((status, body["status"].as_str()), (202, Some("pending")));
    drain(&plain).await;

    let (status, body) = get_json(&app, &format!("/v1/artist/{root}/similar")).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "candidates");
    assert_eq!(body["name"], "Root Band");
    let names: Vec<&str> = body["similar"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    // B: (1.0 + 0.9) / 2, C: (0.8 + 0.4) / 2. The Last.fm entry without an MBID is dropped.
    assert_eq!(names, vec!["Band B", "Band C"]);
    assert_eq!(
        body["similar"][0]["sources"],
        json!(["lastfm", "listenbrainz"])
    );
    assert!(body["similar"][0]["reason"].is_null());

    // With an LLM the same artist gets ranked edges.
    let ranked = state(Some(&fake_llm().await)).await;
    let app = build_router(ranked.clone());
    let (status, body) = get_json(&app, &format!("/v1/artist/{root}/similar")).await;
    assert_eq!((status, body["status"].as_str()), (200, Some("candidates")));
    // The hourly request guard for this artist is already set in Redis, so queue the job directly.
    sqlx::query("INSERT INTO jobs (kind, mbid) VALUES ('similar', $1) ON CONFLICT DO NOTHING")
        .bind(root)
        .execute(&ranked.pool)
        .await
        .unwrap();
    drain(&ranked).await;

    let (status, body) = get_json(&app, &format!("/v1/artist/{root}/similar")).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "ready");
    let similar = body["similar"].as_array().unwrap();
    assert_eq!(similar.len(), 2, "the invented artist is dropped");
    assert_eq!(similar[0]["mbid"], C);
    assert_eq!(similar[0]["kind"], "members");
    assert_eq!(similar[0]["reason"], "Share a drummer.");
    assert_eq!(similar[1]["mbid"], B);

    let (status, graph) = get_json(&app, &format!("/v1/graph/{root}?depth=2")).await;
    assert_eq!(status, 200);
    assert_eq!(graph["status"], "ready");
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(graph["edges"].as_array().unwrap().len(), 2);
    assert_eq!(graph["edges"][0]["from"], root.to_string());
}
