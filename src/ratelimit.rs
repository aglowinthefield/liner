//! Fixed-window rate limiting in Redis, keyed by API key or client IP.
//!
//! Requests with a valid `X-Liner-Key` header count against that key's limit.
//! Everything else counts against the client IP at the anonymous limit.
//! If Redis or Postgres fails, the request is let through.

use std::{net::SocketAddr, time::Duration};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use sha2::{Digest, Sha256};

use crate::AppState;

pub const KEY_HEADER: &str = "x-liner-key";
const WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct RateLimitConfig {
    pub anon_per_min: u32,
    pub key_per_min: u32,
    pub trust_proxy: bool,
}

pub fn hash_key(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Returns the client IP. Behind a proxy, prefers `CF-Connecting-IP`, then the first
/// `X-Forwarded-For` entry. These headers can be forged by anyone who reaches the
/// origin directly, so treat the anonymous limit as best effort.
fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>, trust_proxy: bool) -> String {
    if trust_proxy {
        let from = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        if let Some(ip) = from("cf-connecting-ip") {
            return ip.trim().to_string();
        }
        if let Some(ip) = from("x-forwarded-for").and_then(|v| v.split(',').next()) {
            return ip.trim().to_string();
        }
    }
    peer.map(|p| p.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

async fn key_limit(state: &AppState, key: &str) -> Option<(String, u32)> {
    let row: Option<(i64, Option<i32>)> = sqlx::query_as(
        "SELECT id, rate_limit_per_min FROM api_keys WHERE key_hash = $1 AND revoked_at IS NULL",
    )
    .bind(hash_key(key))
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| tracing::warn!("api key lookup failed: {e}"))
    .ok()
    .flatten();
    row.map(|(id, limit)| {
        (
            format!("key:{id}"),
            limit.map(|l| l as u32).unwrap_or(state.rate.key_per_min),
        )
    })
}

pub async fn middleware(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if path == "/healthz" || path == "/metrics" {
        return next.run(req).await;
    }

    let supplied_key = req
        .headers()
        .get(KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let (subject, limit) = match supplied_key {
        Some(key) => match key_limit(&state, &key).await {
            Some(found) => found,
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": "invalid API key" })),
                )
                    .into_response()
            }
        },
        None => {
            let peer = req
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0);
            let ip = client_ip(req.headers(), peer, state.rate.trust_proxy);
            (format!("ip:{ip}"), state.rate.anon_per_min)
        }
    };

    let minute = chrono::Utc::now().timestamp() / 60;
    let redis_key = format!("liner:rl:{subject}:{minute}");
    match state.cache.incr_window(&redis_key, WINDOW).await {
        Ok(count) if count > u64::from(limit) => {
            let retry = 60 - chrono::Utc::now().timestamp() % 60;
            let mut res = (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({ "error": "rate limit exceeded" })),
            )
                .into_response();
            res.headers_mut()
                .insert(header::RETRY_AFTER, retry.to_string().parse().unwrap());
            res
        }
        Ok(_) => next.run(req).await,
        Err(e) => {
            tracing::warn!("rate limit check failed, allowing request: {e}");
            next.run(req).await
        }
    }
}
