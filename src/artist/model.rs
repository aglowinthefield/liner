use anyhow::Result;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::cache::Cache;

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ArtistDetail {
    pub mbid: Uuid,
    pub name: String,
    pub sort_name: String,
    pub artist_type: Option<String>,
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    pub begin_date: Option<String>,
    pub end_date: Option<String>,
    pub links: Vec<ExternalLink>,
    pub release_groups: Vec<ReleaseGroupSummary>,
}

/// Artist data from MusicBrainz plus bio and images from other sources.
#[derive(Debug, Serialize, ToSchema)]
pub struct ArtistResponse {
    #[serde(flatten)]
    pub artist: ArtistDetail,
    /// `null` when no source has a bio for this artist.
    pub bio: Option<crate::enrich::Bio>,
    pub images: Vec<crate::enrich::Image>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExternalLink {
    pub link_type: String,
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReleaseGroupSummary {
    pub mbid: Uuid,
    pub title: String,
    pub primary_type: Option<String>,
    pub first_release_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsArtistLookup {
    id: Uuid,
    name: String,
    #[serde(default, rename = "sort-name")]
    sort_name: String,
    #[serde(rename = "type")]
    artist_type: Option<String>,
    country: Option<String>,
    disambiguation: Option<String>,
    #[serde(rename = "life-span")]
    life_span: Option<WsLifeSpan>,
    #[serde(default)]
    relations: Vec<WsRelation>,
}

#[derive(Debug, Deserialize)]
struct WsRelation {
    #[serde(rename = "type")]
    relation_type: String,
    url: Option<WsUrl>,
}

#[derive(Debug, Deserialize)]
struct WsUrl {
    resource: String,
}

#[derive(Debug, Deserialize)]
struct WsLifeSpan {
    begin: Option<String>,
    end: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsReleaseGroupBrowse {
    #[serde(rename = "release-groups")]
    release_groups: Vec<WsReleaseGroupEntry>,
}

#[derive(Debug, Deserialize)]
struct WsReleaseGroupEntry {
    id: Uuid,
    title: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
}

pub async fn get_artist(base_url: &str, mbid: Uuid, cache: &Cache) -> Result<ArtistDetail> {
    let base_url = base_url.to_string();
    cache
        .get_or_fetch(&format!("liner:artist:{mbid}"), || fetch_artist(&base_url, mbid))
        .await
}

async fn fetch_artist(base_url: &str, mbid: Uuid) -> Result<ArtistDetail> {
    let client = reqwest::Client::new();

    // Fetch artist details and release groups in parallel
    let (artist_resp, rg_resp) = tokio::join!(
        client
            .get(format!("{base_url}/ws/2/artist/{mbid}"))
            .query(&[("inc", "url-rels"), ("fmt", "json")])
            .send(),
        client
            .get(format!("{base_url}/ws/2/release-group"))
            .query(&[
                ("artist", &mbid.to_string()),
                ("fmt", &"json".to_string()),
                ("limit", &"100".to_string()),
            ])
            .send(),
    );

    let artist: WsArtistLookup = crate::musicbrainz::check(artist_resp?)?.json().await?;
    let rg_browse: WsReleaseGroupBrowse = crate::musicbrainz::check(rg_resp?)?.json().await?;

    let (begin_date, end_date) = match artist.life_span {
        Some(ls) => (ls.begin, ls.end),
        None => (None, None),
    };

    let links: Vec<ExternalLink> = artist
        .relations
        .iter()
        .filter_map(|rel| {
            rel.url.as_ref().map(|url| ExternalLink {
                link_type: rel.relation_type.clone(),
                url: url.resource.clone(),
            })
        })
        .collect();

    let mut release_groups: Vec<ReleaseGroupSummary> = rg_browse
        .release_groups
        .into_iter()
        .map(|rg| ReleaseGroupSummary {
            mbid: rg.id,
            title: rg.title,
            primary_type: rg.primary_type,
            first_release_date: rg.first_release_date,
        })
        .collect();

    // Sort by release date descending (newest first), entries without dates go last
    release_groups.sort_by(|a, b| {
        let da = a.first_release_date.as_deref().unwrap_or("");
        let db = b.first_release_date.as_deref().unwrap_or("");
        db.cmp(da)
    });

    Ok(ArtistDetail {
        mbid: artist.id,
        name: artist.name,
        sort_name: artist.sort_name,
        artist_type: artist.artist_type,
        country: artist.country,
        disambiguation: artist.disambiguation,
        begin_date,
        end_date,
        links,
        release_groups,
    })
}
