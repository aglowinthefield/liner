//! Release group cover art from the Cover Art Archive.

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::cache::Cache;

const PUBLIC_URL: &str = "https://coverartarchive.org";
const HIT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MISS_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub struct CoverArt {
    client: reqwest::Client,
    base_url: String,
}

impl Default for CoverArt {
    fn default() -> Self {
        Self::new(PUBLIC_URL)
    }
}

/// Front cover URLs at three sizes. They redirect to the image, so clients should follow redirects.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct Cover {
    pub small: String,
    pub medium: String,
    pub large: String,
}

impl CoverArt {
    pub fn new(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(super::USER_AGENT)
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Returns the front cover of a release group, or `None` if the archive has none.
    /// A failed check counts as no cover, for a short time.
    pub async fn front(&self, cache: &Cache, mbid: Uuid) -> Option<Cover> {
        let hit_key = format!("liner:caa:hit:{mbid}");
        let miss_key = format!("liner:caa:miss:{mbid}");
        let cover = || Cover {
            small: format!("{PUBLIC_URL}/release-group/{mbid}/front-250"),
            medium: format!("{PUBLIC_URL}/release-group/{mbid}/front-500"),
            large: format!("{PUBLIC_URL}/release-group/{mbid}/front-1200"),
        };
        if cache.is_marked(&hit_key).await {
            return Some(cover());
        }
        if cache.is_marked(&miss_key).await {
            return None;
        }
        match self.exists(mbid).await {
            Ok(true) => {
                cache.mark(&hit_key, HIT_TTL).await;
                Some(cover())
            }
            Ok(false) => {
                cache.mark(&miss_key, MISS_TTL).await;
                None
            }
            Err(e) => {
                tracing::warn!("cover art check for {mbid} failed: {e}");
                cache.mark(&miss_key, Duration::from_secs(60)).await;
                None
            }
        }
    }

    async fn exists(&self, mbid: Uuid) -> Result<bool> {
        let resp = self
            .client
            .head(format!("{}/release-group/{mbid}/front-250", self.base_url))
            .send()
            .await?;
        let status = resp.status();
        if status.is_redirection() || status.is_success() {
            Ok(true)
        } else if status == reqwest::StatusCode::NOT_FOUND {
            Ok(false)
        } else {
            anyhow::bail!("unexpected status {status}")
        }
    }
}
