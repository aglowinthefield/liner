//! Bios, tags, and images stored in Postgres, filled from outside sources on first request.
//!
//! Enrichment is best effort: any failure is logged and the artist is served without it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    artist::model::ExternalLink,
    sources::{lastfm, wikipedia::qid_from_url},
    AppState,
};

const WIKIPEDIA_LICENSE: &str = "CC BY-SA 4.0";
const MAX_AGE_DAYS: i32 = 30;
const MISS_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const ERROR_TTL: Duration = Duration::from_secs(5 * 60);
const FANART_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Bio {
    /// `wikipedia` or `lastfm`.
    pub source: String,
    pub lang: String,
    pub summary: String,
    /// Page the text came from.
    pub url: Option<String>,
    /// License of the text. Show it, and a link to `url`, wherever you show the bio.
    pub license: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Image {
    /// `thumbnail` (about 330 px wide), `primary`, `background`, or `logo`.
    pub kind: String,
    pub url: String,
    /// `wikipedia` or `fanart`.
    pub source: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

#[derive(Debug, Default)]
pub struct Enrichment {
    pub bio: Option<Bio>,
    pub images: Vec<Image>,
    pub tags: Vec<String>,
}

pub async fn artist(state: &AppState, mbid: Uuid, links: &[ExternalLink]) -> Enrichment {
    let qid = links
        .iter()
        .filter(|l| l.link_type == "wikidata")
        .find_map(|l| qid_from_url(&l.url));
    tokio::join!(
        ensure_wikipedia(state, mbid, qid),
        ensure_lastfm(state, mbid),
        ensure_fanart(state, mbid)
    );
    match load(state, mbid).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!("loading stored enrichment for {mbid} failed: {e}");
            Enrichment::default()
        }
    }
}

async fn bio_is_fresh(state: &AppState, mbid: Uuid, source: &str) -> bool {
    sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM artist_bios WHERE mbid = $1 AND source = $2 AND fetched_at > now() - make_interval(days => $3)",
    )
    .bind(mbid)
    .bind(source)
    .bind(MAX_AGE_DAYS)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None)
    .is_some()
}

async fn ensure_wikipedia(state: &AppState, mbid: Uuid, qid: Option<&str>) {
    let Some(qid) = qid else { return };
    let miss_key = format!("liner:enrich:miss:wikipedia:{mbid}");
    if bio_is_fresh(state, mbid, "wikipedia").await || state.cache.is_marked(&miss_key).await {
        return;
    }
    match state.wiki.summary_for_qid(qid).await {
        Ok(Some(s)) => {
            let bio = Bio {
                source: "wikipedia".into(),
                lang: "en".into(),
                summary: s.extract,
                url: Some(s.url),
                license: Some(WIKIPEDIA_LICENSE.into()),
            };
            let images: Vec<Image> = [("thumbnail", s.thumbnail), ("primary", s.original)]
                .into_iter()
                .filter_map(|(kind, p)| {
                    p.map(|p| Image {
                        kind: kind.into(),
                        url: p.source,
                        source: "wikipedia".into(),
                        width: p.width,
                        height: p.height,
                    })
                })
                .collect();
            if let Err(e) = store(state, mbid, "wikipedia", Some(&bio), &images, None).await {
                tracing::warn!("storing wikipedia enrichment for {mbid} failed: {e}");
            }
        }
        Ok(None) => state.cache.mark(&miss_key, MISS_TTL).await,
        Err(e) => {
            tracing::warn!("wikipedia lookup for {mbid} ({qid}) failed: {e}");
            state.cache.mark(&miss_key, ERROR_TTL).await;
        }
    }
}

async fn ensure_lastfm(state: &AppState, mbid: Uuid) {
    let Some(lastfm) = &state.lastfm else { return };
    let miss_key = format!("liner:enrich:miss:lastfm:{mbid}");
    if bio_is_fresh(state, mbid, "lastfm").await || state.cache.is_marked(&miss_key).await {
        return;
    }
    match lastfm.artist_info(mbid).await {
        Ok(Some(info)) => {
            let bio = (!info.summary.is_empty()).then(|| Bio {
                source: "lastfm".into(),
                lang: "en".into(),
                summary: info.summary,
                url: Some(info.url).filter(|u| !u.is_empty()),
                license: Some(lastfm::LICENSE.into()),
            });
            if bio.is_none() && info.tags.is_empty() {
                state.cache.mark(&miss_key, MISS_TTL).await;
            } else if let Err(e) =
                store(state, mbid, "lastfm", bio.as_ref(), &[], Some(&info.tags)).await
            {
                tracing::warn!("storing last.fm enrichment for {mbid} failed: {e}");
            }
        }
        Ok(None) => state.cache.mark(&miss_key, MISS_TTL).await,
        Err(e) => {
            tracing::warn!("last.fm lookup for {mbid} failed: {e}");
            state.cache.mark(&miss_key, ERROR_TTL).await;
        }
    }
}

async fn ensure_fanart(state: &AppState, mbid: Uuid) {
    let Some(fanart) = &state.fanart else { return };
    let done_key = format!("liner:enrich:done:fanart:{mbid}");
    if state.cache.is_marked(&done_key).await {
        return;
    }
    match fanart.artist_images(mbid).await {
        Ok(found) => {
            let images: Vec<Image> = found
                .into_iter()
                .map(|i| Image {
                    kind: i.kind.into(),
                    url: i.url,
                    source: "fanart".into(),
                    width: None,
                    height: None,
                })
                .collect();
            if let Err(e) = store(state, mbid, "fanart", None, &images, None).await {
                tracing::warn!("storing fanart.tv images for {mbid} failed: {e}");
            } else {
                state.cache.mark(&done_key, FANART_TTL).await;
            }
        }
        Err(e) => {
            tracing::warn!("fanart.tv lookup for {mbid} failed: {e}");
            state.cache.mark(&done_key, ERROR_TTL).await;
        }
    }
}

async fn load(state: &AppState, mbid: Uuid) -> anyhow::Result<Enrichment> {
    let bios: Vec<Bio> = sqlx::query_as(
        "SELECT source, lang, summary, url, license FROM artist_bios WHERE mbid = $1",
    )
    .bind(mbid)
    .fetch_all(&state.pool)
    .await?;
    let images: Vec<Image> = sqlx::query_as(
        "SELECT kind, url, source, width, height FROM images WHERE mbid = $1 AND entity_type = 'artist'
         ORDER BY source, kind DESC",
    )
    .bind(mbid)
    .fetch_all(&state.pool)
    .await?;
    let tags: Vec<String> =
        sqlx::query_scalar("SELECT name FROM artist_tags WHERE mbid = $1 ORDER BY position")
            .bind(mbid)
            .fetch_all(&state.pool)
            .await?;
    // Prefer the Wikipedia bio: it is usually longer and better sourced.
    let bio = bios
        .iter()
        .find(|b| b.source == "wikipedia")
        .or_else(|| bios.first())
        .cloned();
    Ok(Enrichment { bio, images, tags })
}

/// Replaces what `source` previously stored for the artist.
async fn store(
    state: &AppState,
    mbid: Uuid,
    source: &str,
    bio: Option<&Bio>,
    images: &[Image],
    tags: Option<&[String]>,
) -> anyhow::Result<()> {
    let mut tx = state.pool.begin().await?;
    if let Some(b) = bio {
        sqlx::query(
            "INSERT INTO artist_bios (mbid, source, lang, summary, url, license)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (mbid, source, lang) DO UPDATE
             SET summary = EXCLUDED.summary, url = EXCLUDED.url, license = EXCLUDED.license, fetched_at = now()",
        )
        .bind(mbid)
        .bind(&b.source)
        .bind(&b.lang)
        .bind(&b.summary)
        .bind(&b.url)
        .bind(&b.license)
        .execute(&mut *tx)
        .await?;
    }
    if source != "lastfm" {
        sqlx::query(
            "DELETE FROM images WHERE mbid = $1 AND entity_type = 'artist' AND source = $2",
        )
        .bind(mbid)
        .bind(source)
        .execute(&mut *tx)
        .await?;
    }
    for i in images {
        sqlx::query(
            "INSERT INTO images (mbid, entity_type, kind, url, source, width, height)
             VALUES ($1, 'artist', $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
        )
        .bind(mbid)
        .bind(&i.kind)
        .bind(&i.url)
        .bind(&i.source)
        .bind(i.width)
        .bind(i.height)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(tags) = tags {
        sqlx::query("DELETE FROM artist_tags WHERE mbid = $1 AND source = $2")
            .bind(mbid)
            .bind(source)
            .execute(&mut *tx)
            .await?;
        for (position, name) in tags.iter().enumerate() {
            sqlx::query("INSERT INTO artist_tags (mbid, source, name, position) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
                .bind(mbid)
                .bind(source)
                .bind(name)
                .bind(position as i32)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
