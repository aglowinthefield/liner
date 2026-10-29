//! Bios and images stored in Postgres, filled from outside sources on first request.
//!
//! Enrichment is best effort: any failure is logged and the artist is served without it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    artist::model::ExternalLink,
    sources::wikipedia::{qid_from_url, Summary},
    AppState,
};

const WIKIPEDIA_LICENSE: &str = "CC BY-SA 4.0";
const BIO_MAX_AGE_DAYS: i32 = 30;
const MISS_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const ERROR_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, sqlx::FromRow)]
pub struct Bio {
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
    /// `thumbnail` (about 330 px wide) or `primary` (original size).
    pub kind: String,
    pub url: String,
    pub source: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

#[derive(Debug, Default)]
pub struct Enrichment {
    pub bio: Option<Bio>,
    pub images: Vec<Image>,
}

pub async fn artist(state: &AppState, mbid: Uuid, links: &[ExternalLink]) -> Enrichment {
    match load(state, mbid).await {
        Ok(Some(found)) => return found,
        Ok(None) => {}
        Err(e) => tracing::warn!("loading stored enrichment for {mbid} failed: {e}"),
    }

    let Some(qid) = links.iter().filter(|l| l.link_type == "wikidata").find_map(|l| qid_from_url(&l.url)) else {
        return Enrichment::default();
    };

    let miss_key = format!("liner:enrich:miss:{mbid}");
    if state.cache.is_marked(&miss_key).await {
        return Enrichment::default();
    }

    match state.wiki.summary_for_qid(qid).await {
        Ok(Some(summary)) => {
            let enrichment = from_summary(&summary);
            if let Err(e) = store(state, mbid, &enrichment).await {
                tracing::warn!("storing enrichment for {mbid} failed: {e}");
            }
            enrichment
        }
        Ok(None) => {
            state.cache.mark(&miss_key, MISS_TTL).await;
            Enrichment::default()
        }
        Err(e) => {
            tracing::warn!("wikipedia lookup for {mbid} ({qid}) failed: {e}");
            state.cache.mark(&miss_key, ERROR_TTL).await;
            Enrichment::default()
        }
    }
}

fn from_summary(s: &Summary) -> Enrichment {
    let mut images = Vec::new();
    if let Some(p) = &s.thumbnail {
        images.push(Image {
            kind: "thumbnail".into(),
            url: p.source.clone(),
            source: "wikipedia".into(),
            width: p.width,
            height: p.height,
        });
    }
    if let Some(p) = &s.original {
        images.push(Image {
            kind: "primary".into(),
            url: p.source.clone(),
            source: "wikipedia".into(),
            width: p.width,
            height: p.height,
        });
    }
    Enrichment {
        bio: Some(Bio {
            source: "wikipedia".into(),
            lang: "en".into(),
            summary: s.extract.clone(),
            url: Some(s.url.clone()),
            license: Some(WIKIPEDIA_LICENSE.into()),
        }),
        images,
    }
}

async fn load(state: &AppState, mbid: Uuid) -> anyhow::Result<Option<Enrichment>> {
    let bio: Option<Bio> = sqlx::query_as(
        "SELECT source, lang, summary, url, license FROM artist_bios
         WHERE mbid = $1 AND source = 'wikipedia' AND lang = 'en'
           AND fetched_at > now() - make_interval(days => $2)",
    )
    .bind(mbid)
    .bind(BIO_MAX_AGE_DAYS)
    .fetch_optional(&state.pool)
    .await?;
    let Some(bio) = bio else { return Ok(None) };

    let images: Vec<Image> = sqlx::query_as(
        "SELECT kind, url, source, width, height FROM images
         WHERE mbid = $1 AND entity_type = 'artist' AND source = 'wikipedia' ORDER BY kind DESC",
    )
    .bind(mbid)
    .fetch_all(&state.pool)
    .await?;
    Ok(Some(Enrichment { bio: Some(bio), images }))
}

async fn store(state: &AppState, mbid: Uuid, e: &Enrichment) -> anyhow::Result<()> {
    let mut tx = state.pool.begin().await?;
    if let Some(b) = &e.bio {
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
    sqlx::query("DELETE FROM images WHERE mbid = $1 AND entity_type = 'artist' AND source = 'wikipedia'")
        .bind(mbid)
        .execute(&mut *tx)
        .await?;
    for i in &e.images {
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
    tx.commit().await?;
    Ok(())
}
