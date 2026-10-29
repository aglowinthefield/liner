use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::cache::Cache;

// --- Public response types ---

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReleaseGroupDetail {
    pub mbid: Uuid,
    pub title: String,
    pub primary_type: Option<String>,
    pub secondary_types: Vec<String>,
    pub first_release_date: Option<String>,
    pub disambiguation: Option<String>,
    pub artists: Vec<ArtistCredit>,
    pub links: Vec<ExternalLink>,
    pub labels: Vec<LabelInfo>,
    pub media: Vec<Medium>,
    pub release_mbid: Option<Uuid>,
    pub release_country: Option<String>,
    pub release_date: Option<String>,
    pub release_status: Option<String>,
    pub barcode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ArtistCredit {
    pub mbid: Uuid,
    pub name: String,
    pub join_phrase: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExternalLink {
    pub link_type: String,
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LabelInfo {
    pub name: String,
    pub mbid: Option<Uuid>,
    pub catalog_number: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Medium {
    pub position: u32,
    pub format: Option<String>,
    pub title: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Track {
    pub position: u32,
    pub number: String,
    pub title: String,
    pub length_ms: Option<u64>,
    pub artist_credit: Option<Vec<ArtistCredit>>,
}

// --- MusicBrainz ws/2 JSON shapes ---

#[derive(Debug, Deserialize)]
struct WsReleaseGroup {
    id: Uuid,
    title: String,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(default, rename = "secondary-types")]
    secondary_types: Vec<String>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
    disambiguation: Option<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<WsArtistCredit>,
    #[serde(default)]
    releases: Vec<WsReleaseRef>,
    #[serde(default)]
    relations: Vec<WsRelation>,
}

#[derive(Debug, Deserialize)]
struct WsArtistCredit {
    artist: WsArtistRef,
    #[serde(default)]
    joinphrase: String,
}

#[derive(Debug, Deserialize)]
struct WsArtistRef {
    id: Uuid,
    name: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WsReleaseRef {
    id: Uuid,
    status: Option<String>,
    date: Option<String>,
    country: Option<String>,
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
struct WsRelease {
    id: Uuid,
    status: Option<String>,
    date: Option<String>,
    country: Option<String>,
    barcode: Option<String>,
    #[serde(default, rename = "label-info")]
    label_info: Vec<WsLabelInfo>,
    #[serde(default)]
    media: Vec<WsMedium>,
    #[serde(default)]
    relations: Vec<WsRelation>,
}

#[derive(Debug, Deserialize)]
struct WsLabelInfo {
    #[serde(rename = "catalog-number")]
    catalog_number: Option<String>,
    label: Option<WsLabel>,
}

#[derive(Debug, Deserialize)]
struct WsLabel {
    id: Uuid,
    name: String,
}

#[derive(Debug, Deserialize)]
struct WsMedium {
    position: u32,
    format: Option<String>,
    title: Option<String>,
    #[serde(default)]
    tracks: Vec<WsTrack>,
}

#[derive(Debug, Deserialize)]
struct WsTrack {
    position: u32,
    number: String,
    title: String,
    length: Option<u64>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<WsArtistCredit>,
}

pub async fn get_release_group(base_url: &str, mbid: Uuid, cache: &Cache) -> Result<ReleaseGroupDetail> {
    let base_url = base_url.to_string();
    cache
        .get_or_fetch(&format!("liner:rg:{mbid}"), || fetch_release_group(&base_url, mbid))
        .await
}

async fn fetch_release_group(base_url: &str, mbid: Uuid) -> Result<ReleaseGroupDetail> {
    let client = reqwest::Client::new();

    // Step 1: Fetch release group with artist credits, releases list, and URL rels
    let rg: WsReleaseGroup = client
        .get(format!("{base_url}/ws/2/release-group/{mbid}"))
        .query(&[
            ("inc", "artist-credits+releases+url-rels"),
            ("fmt", "json"),
        ])
        .send()
        .await?
        .json()
        .await?;

    let artists: Vec<ArtistCredit> = rg
        .artist_credit
        .iter()
        .map(|ac| ArtistCredit {
            mbid: ac.artist.id,
            name: ac.artist.name.clone(),
            join_phrase: ac.joinphrase.clone(),
        })
        .collect();

    let rg_links = extract_links(&rg.relations);

    // Step 2: Pick the best release (prefer "Official" status, then earliest date)
    let best_release = rg
        .releases
        .iter()
        .min_by_key(|r| {
            let status_rank = match r.status.as_deref() {
                Some("Official") => 0,
                _ => 1,
            };
            let date = r.date.clone().unwrap_or_default();
            (status_rank, date)
        })
        .ok_or_else(|| anyhow!("No releases found for release group"))?;

    // Step 3: Fetch the full release with recordings, labels, and URL rels
    let release: WsRelease = client
        .get(format!("{base_url}/ws/2/release/{}", best_release.id))
        .query(&[
            ("inc", "recordings+media+labels+artist-credits+url-rels"),
            ("fmt", "json"),
        ])
        .send()
        .await?
        .json()
        .await?;

    let release_links = extract_links(&release.relations);

    // Merge links from release group and release, dedup by URL
    let mut all_links = rg_links;
    for link in release_links {
        if !all_links.iter().any(|l| l.url == link.url) {
            all_links.push(link);
        }
    }

    let labels: Vec<LabelInfo> = release
        .label_info
        .into_iter()
        .filter_map(|li| {
            li.label.map(|l| LabelInfo {
                name: l.name,
                mbid: Some(l.id),
                catalog_number: li.catalog_number,
            })
        })
        .collect();

    let media: Vec<Medium> = release
        .media
        .into_iter()
        .map(|m| Medium {
            position: m.position,
            format: m.format,
            title: m.title,
            tracks: m
                .tracks
                .into_iter()
                .map(|t| {
                    let track_artists: Vec<ArtistCredit> = t
                        .artist_credit
                        .iter()
                        .map(|ac| ArtistCredit {
                            mbid: ac.artist.id,
                            name: ac.artist.name.clone(),
                            join_phrase: ac.joinphrase.clone(),
                        })
                        .collect();

                    Track {
                        position: t.position,
                        number: t.number,
                        title: t.title,
                        length_ms: t.length,
                        artist_credit: if track_artists.is_empty() {
                            None
                        } else {
                            Some(track_artists)
                        },
                    }
                })
                .collect(),
        })
        .collect();

    Ok(ReleaseGroupDetail {
        mbid: rg.id,
        title: rg.title,
        primary_type: rg.primary_type,
        secondary_types: rg.secondary_types,
        first_release_date: rg.first_release_date,
        disambiguation: rg.disambiguation,
        artists,
        links: all_links,
        labels,
        media,
        release_mbid: Some(release.id),
        release_country: release.country,
        release_date: release.date,
        release_status: release.status,
        barcode: release.barcode,
    })
}

fn extract_links(relations: &[WsRelation]) -> Vec<ExternalLink> {
    relations
        .iter()
        .filter_map(|rel| {
            rel.url.as_ref().map(|url| ExternalLink {
                link_type: rel.relation_type.clone(),
                url: url.resource.clone(),
            })
        })
        .collect()
}
