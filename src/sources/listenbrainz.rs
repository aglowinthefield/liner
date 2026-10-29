//! Similar artists from ListenBrainz Labs, based on what people listen to in the same sessions.

use anyhow::Result;
use serde::Deserialize;
use uuid::Uuid;

const ALGORITHM: &str =
    "session_based_days_9000_session_300_contribution_5_threshold_15_limit_50_skip_30";

#[derive(Clone)]
pub struct ListenBrainz {
    client: reqwest::Client,
    base_url: String,
}

impl Default for ListenBrainz {
    fn default() -> Self {
        Self::new("https://labs.api.listenbrainz.org")
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SimilarArtist {
    pub artist_mbid: Uuid,
    pub name: String,
    pub score: f64,
}

impl ListenBrainz {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: super::http_client(8),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Returns similar artists, most similar first. Scores are raw session counts.
    pub async fn similar(&self, mbid: Uuid) -> Result<Vec<SimilarArtist>> {
        let mut found: Vec<SimilarArtist> = self
            .client
            .get(format!("{}/similar-artists/json", self.base_url))
            .query(&[
                ("artist_mbids", mbid.to_string().as_str()),
                ("algorithm", ALGORITHM),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        found.retain(|a| a.artist_mbid != mbid);
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(found)
    }
}
