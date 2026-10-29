use anyhow::Result;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::cache::Cache;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SearchResponse {
    pub artists: Vec<ArtistResult>,
    pub release_groups: Vec<ReleaseGroupResult>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ArtistResult {
    pub mbid: Uuid,
    pub name: String,
    pub sort_name: String,
    pub artist_type: Option<String>,
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    pub begin_year: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReleaseGroupResult {
    pub mbid: Uuid,
    pub title: String,
    pub artist_name: String,
    pub release_group_type: Option<String>,
    pub first_release_date: Option<String>,
}

// ws/2 JSON response shapes

#[derive(Debug, Deserialize)]
struct WsArtistResponse {
    artists: Vec<WsArtist>,
}

#[derive(Debug, Deserialize)]
struct WsArtist {
    id: Uuid,
    name: String,
    #[serde(default, rename = "sort-name")]
    sort_name: String,
    #[serde(rename = "type")]
    artist_type: Option<String>,
    country: Option<String>,
    disambiguation: Option<String>,
    #[serde(default, rename = "life-span")]
    life_span: Option<WsLifeSpan>,
}

#[derive(Debug, Deserialize)]
struct WsLifeSpan {
    begin: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsReleaseGroupResponse {
    #[serde(rename = "release-groups")]
    release_groups: Vec<WsReleaseGroup>,
}

#[derive(Debug, Deserialize)]
struct WsReleaseGroup {
    id: Uuid,
    title: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<WsArtistCredit>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsArtistCredit {
    name: String,
}

pub async fn search(base_url: &str, query: &str, cache: &Cache) -> Result<SearchResponse> {
    let base_url = base_url.to_string();
    let query = query.to_string();
    cache
        .get_or_fetch_search(&format!("liner:search:{query}"), || {
            fetch_search(&base_url, &query)
        })
        .await
}

async fn fetch_search(base_url: &str, query: &str) -> Result<SearchResponse> {
    let (artists, release_groups) = tokio::join!(
        fetch_artists(base_url, query),
        fetch_release_groups(base_url, query),
    );
    Ok(SearchResponse {
        artists: artists?,
        release_groups: release_groups?,
    })
}

async fn fetch_artists(base_url: &str, query: &str) -> Result<Vec<ArtistResult>> {
    let client = reqwest::Client::new();
    let resp: WsArtistResponse = client
        .get(format!("{base_url}/ws/2/artist"))
        .query(&[("query", query), ("fmt", "json"), ("limit", "10")])
        .send()
        .await?
        .json()
        .await?;

    Ok(resp
        .artists
        .into_iter()
        .map(|a| {
            let begin_year = a.life_span.and_then(|ls| {
                ls.begin
                    .and_then(|b| b.split('-').next().map(|y| y.to_string()))
            });
            ArtistResult {
                mbid: a.id,
                name: a.name,
                sort_name: a.sort_name,
                artist_type: a.artist_type,
                country: a.country,
                disambiguation: a.disambiguation,
                begin_year,
            }
        })
        .collect())
}

async fn fetch_release_groups(base_url: &str, query: &str) -> Result<Vec<ReleaseGroupResult>> {
    let client = reqwest::Client::new();
    let resp: WsReleaseGroupResponse = client
        .get(format!("{base_url}/ws/2/release-group"))
        .query(&[("query", query), ("fmt", "json"), ("limit", "10")])
        .send()
        .await?
        .json()
        .await?;

    Ok(resp
        .release_groups
        .into_iter()
        .map(|rg| {
            let artist_name = rg
                .artist_credit
                .into_iter()
                .map(|ac| ac.name)
                .collect::<Vec<_>>()
                .join(", ");
            let first_release_date = rg.first_release_date.filter(|d| !d.is_empty());
            ReleaseGroupResult {
                mbid: rg.id,
                title: rg.title,
                artist_name,
                release_group_type: rg.primary_type,
                first_release_date,
            }
        })
        .collect())
}
