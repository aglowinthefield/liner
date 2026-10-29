//! Artist images from fanart.tv. Needs an API key.

use anyhow::Result;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Clone)]
pub struct Fanart {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FanartImage {
    /// `primary`, `background`, or `logo`.
    pub kind: &'static str,
    pub url: String,
}

#[derive(Deserialize, Default)]
struct Response {
    #[serde(default)]
    artistthumb: Vec<Entry>,
    #[serde(default)]
    artistbackground: Vec<Entry>,
    #[serde(default)]
    hdmusiclogo: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    url: String,
}

impl Fanart {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            client: super::http_client(4),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }

    pub fn production(api_key: &str) -> Self {
        Self::new("https://webservice.fanart.tv", api_key)
    }

    /// Returns the first image of each kind that fanart.tv has for the artist.
    pub async fn artist_images(&self, mbid: Uuid) -> Result<Vec<FanartImage>> {
        let request = self
            .client
            .get(format!("{}/v3/music/{mbid}", self.base_url))
            .query(&[("api_key", &self.api_key)]);
        let resp = super::send("fanart", request).await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(vec![]);
        }
        let body: Response = resp.error_for_status()?.json().await?;
        let mut out = Vec::new();
        for (kind, entries) in [
            ("primary", body.artistthumb),
            ("background", body.artistbackground),
            ("logo", body.hdmusiclogo),
        ] {
            if let Some(e) = entries.into_iter().next() {
                out.push(FanartImage { kind, url: e.url });
            }
        }
        Ok(out)
    }
}
