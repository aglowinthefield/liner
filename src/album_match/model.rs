use anyhow::Result;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::cache::Cache;

#[derive(Debug, Deserialize, ToSchema)]
pub struct MatchRequest {
    pub albums: Vec<AlbumToMatch>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AlbumToMatch {
    pub artist_name: String,
    pub album_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MatchResponse {
    pub matches: Vec<AlbumMatch>,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct AlbumMatch {
    pub artist_name: String,
    pub album_name: String,
    pub status: MatchStatus,
    /// Release group MBID of the best candidate. Only set when `status` is `confident`.
    pub release_group_mbid: Option<String>,
    /// Artist MBID of the best candidate. Only set when `status` is `confident`.
    pub artist_mbid: Option<Uuid>,
    pub candidates: Vec<MatchCandidate>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchStatus {
    Confident,
    Ambiguous,
    NotFound,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct MatchCandidate {
    pub mbid: String,
    pub title: String,
    pub artist_name: String,
    /// MBID of the first credited artist. Use it with `/v1/artist/{mbid}`.
    #[serde(default)]
    pub artist_mbid: Option<Uuid>,
    pub release_group_type: Option<String>,
    pub first_release_date: Option<String>,
    pub score: u8,
}

// --- MB search response shapes ---

#[derive(Debug, Deserialize)]
struct MbReleaseGroupSearchResponse {
    #[serde(rename = "release-groups", default)]
    release_groups: Vec<MbReleaseGroupHit>,
}

#[derive(Debug, Deserialize)]
struct MbReleaseGroupHit {
    id: Uuid,
    title: String,
    score: u8,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<MbArtistCredit>,
}

#[derive(Debug, Deserialize)]
struct MbArtistCredit {
    name: String,
    #[serde(default)]
    joinphrase: Option<String>,
    artist: Option<MbCreditedArtist>,
}

#[derive(Debug, Deserialize)]
struct MbCreditedArtist {
    id: Uuid,
}

/// Search MusicBrainz for a single (artist, album) pair and return ranked candidates.
pub async fn match_album(
    base_url: &str,
    cache: &Cache,
    artist_name: &str,
    album_name: &str,
) -> AlbumMatch {
    let cache_key = format!(
        "liner:match:v2:{}:{}",
        artist_name.to_lowercase().trim(),
        album_name.to_lowercase().trim()
    );

    let candidates = cache
        .get_or_fetch(&cache_key, || {
            fetch_candidates(base_url, artist_name, album_name)
        })
        .await
        .unwrap_or_default();

    let status = if candidates.is_empty() {
        MatchStatus::NotFound
    } else if candidates[0].score >= 90
        && candidates[0].title.to_lowercase() == album_name.to_lowercase().trim()
    {
        MatchStatus::Confident
    } else if candidates[0].score >= 70 {
        MatchStatus::Ambiguous
    } else {
        MatchStatus::NotFound
    };

    let best = candidates
        .first()
        .filter(|_| status == MatchStatus::Confident);
    AlbumMatch {
        artist_name: artist_name.to_string(),
        album_name: album_name.to_string(),
        release_group_mbid: best.map(|c| c.mbid.clone()),
        artist_mbid: best.and_then(|c| c.artist_mbid),
        status,
        candidates,
    }
}

async fn fetch_candidates(
    base_url: &str,
    artist_name: &str,
    album_name: &str,
) -> Result<Vec<MatchCandidate>> {
    let query = format!(
        "\"{}\" AND artist:\"{}\"",
        album_name.replace('"', ""),
        artist_name.replace('"', "")
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base_url}/ws/2/release-group"))
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
        .send()
        .await?;

    let body: MbReleaseGroupSearchResponse = resp.json().await?;

    let candidates: Vec<MatchCandidate> = body
        .release_groups
        .into_iter()
        .take(3)
        .map(|rg| {
            let artist_mbid = rg
                .artist_credit
                .first()
                .and_then(|ac| ac.artist.as_ref())
                .map(|a| a.id);
            let artist = rg
                .artist_credit
                .into_iter()
                .map(|ac| {
                    let join = ac.joinphrase.unwrap_or_default();
                    format!("{}{}", ac.name, join)
                })
                .collect::<String>();

            MatchCandidate {
                mbid: rg.id.to_string(),
                title: rg.title,
                artist_name: artist,
                artist_mbid,
                release_group_type: rg.primary_type,
                first_release_date: rg.first_release_date.filter(|d| !d.is_empty()),
                score: rg.score,
            }
        })
        .collect();

    Ok(candidates)
}
