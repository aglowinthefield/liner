use anyhow::Result;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::cache::Cache;

// --- Public response types ---

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LabelDetail {
    pub mbid: Uuid,
    pub name: String,
    pub label_type: Option<String>,
    pub country: Option<String>,
    pub disambiguation: Option<String>,
    pub label_code: Option<u32>,
    pub begin_date: Option<String>,
    pub end_date: Option<String>,
    pub releases: Vec<LabelRelease>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LabelRelease {
    pub release_group_mbid: Option<Uuid>,
    pub title: String,
    pub date: Option<String>,
    pub country: Option<String>,
    pub status: Option<String>,
    pub catalog_number: Option<String>,
    pub artists: Vec<LabelReleaseArtist>,
    pub release_group_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LabelReleaseArtist {
    pub mbid: Uuid,
    pub name: String,
    pub join_phrase: String,
}

// --- MusicBrainz ws/2 JSON shapes ---

#[derive(Debug, Deserialize)]
struct WsLabelLookup {
    id: Uuid,
    name: String,
    #[serde(rename = "type")]
    label_type: Option<String>,
    country: Option<String>,
    disambiguation: Option<String>,
    #[serde(rename = "label-code")]
    label_code: Option<u32>,
    #[serde(rename = "life-span")]
    life_span: Option<WsLifeSpan>,
}

#[derive(Debug, Deserialize)]
struct WsLifeSpan {
    begin: Option<String>,
    end: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsReleaseBrowse {
    releases: Vec<WsRelease>,
}

#[derive(Debug, Deserialize)]
struct WsRelease {
    title: String,
    date: Option<String>,
    country: Option<String>,
    status: Option<String>,
    #[serde(default, rename = "label-info")]
    label_info: Vec<WsLabelInfo>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<WsArtistCredit>,
    #[serde(default, rename = "release-group")]
    release_group: Option<WsReleaseGroup>,
}

#[derive(Debug, Deserialize)]
struct WsLabelInfo {
    #[serde(rename = "catalog-number")]
    catalog_number: Option<String>,
    label: Option<WsLabelRef>,
}

#[derive(Debug, Deserialize)]
struct WsLabelRef {
    id: Uuid,
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
struct WsReleaseGroup {
    id: Uuid,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
}

pub async fn get_label(base_url: &str, mbid: Uuid, cache: &Cache) -> Result<LabelDetail> {
    let base_url = base_url.to_string();
    cache
        .get_or_fetch(&format!("liner:label:{mbid}"), || fetch_label(&base_url, mbid))
        .await
}

async fn fetch_label(base_url: &str, mbid: Uuid) -> Result<LabelDetail> {
    let client = reqwest::Client::new();

    let (label_resp, releases_resp) = tokio::join!(
        client
            .get(format!("{base_url}/ws/2/label/{mbid}"))
            .query(&[("fmt", "json")])
            .send(),
        client
            .get(format!("{base_url}/ws/2/release"))
            .query(&[
                ("label", &mbid.to_string()),
                ("inc", &"artist-credits+release-groups+labels".to_string()),
                ("fmt", &"json".to_string()),
                ("limit", &"100".to_string()),
            ])
            .send(),
    );

    let label: WsLabelLookup = label_resp?.json().await?;
    let browse: WsReleaseBrowse = releases_resp?.json().await?;

    let (begin_date, end_date) = match label.life_span {
        Some(ls) => (ls.begin, ls.end),
        None => (None, None),
    };

    let mut releases: Vec<LabelRelease> = browse
        .releases
        .into_iter()
        .map(|r| {
            let catalog_number = r
                .label_info
                .iter()
                .find(|li| li.label.as_ref().is_some_and(|l| l.id == mbid))
                .and_then(|li| li.catalog_number.clone());

            let artists = r
                .artist_credit
                .into_iter()
                .map(|ac| LabelReleaseArtist {
                    mbid: ac.artist.id,
                    name: ac.artist.name,
                    join_phrase: ac.joinphrase,
                })
                .collect();

            LabelRelease {
                release_group_mbid: r.release_group.as_ref().map(|rg| rg.id),
                title: r.title,
                date: r.date,
                country: r.country,
                status: r.status,
                catalog_number,
                artists,
                release_group_type: r.release_group.and_then(|rg| rg.primary_type),
            }
        })
        .collect();

    // Sort by date descending, missing dates last
    releases.sort_by(|a, b| {
        let da = a.date.as_deref().unwrap_or("");
        let db = b.date.as_deref().unwrap_or("");
        db.cmp(da)
    });

    Ok(LabelDetail {
        mbid: label.id,
        name: label.name,
        label_type: label.label_type,
        country: label.country,
        disambiguation: label.disambiguation,
        label_code: label.label_code,
        begin_date,
        end_date,
        releases,
    })
}
