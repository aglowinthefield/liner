//! Bios, tags, and images stored in Postgres. A request reads what is stored and queues an
//! `enrich` job when it is missing or old; the worker fetches from the outside sources.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    artist::model::get_artist,
    jobs,
    sources::{lastfm, wikipedia::qid_from_url, RateLimited},
    AppState,
};

const WIKIPEDIA_LICENSE: &str = "CC BY-SA 4.0";
const MAX_AGE_DAYS: i32 = 30;
const MISS_TTL: Duration = Duration::from_secs(6 * 60 * 60);
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

/// Whether the worker has finished fetching bios, tags, and images for an artist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnrichmentStatus {
    /// A job is queued or running. Ask again in a few seconds.
    Pending,
    /// Everything liner can find is in the response.
    Complete,
    /// The last job failed. Liner retries after an hour.
    Failed,
}

const FAILED_RETRY_SECS: f64 = 60.0 * 60.0;

/// What is stored for the artist now. Queues a job to fetch more if nothing recent has run,
/// so the response never waits on Wikidata, Wikipedia, Last.fm, or fanart.tv.
pub async fn lookup(state: &AppState, mbid: Uuid) -> (Enrichment, EnrichmentStatus) {
    let stored = match load(state, mbid).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!("loading stored enrichment for {mbid} failed: {e}");
            Enrichment::default()
        }
    };
    match status(state, mbid).await {
        Ok(s) => (stored, s),
        Err(e) => {
            tracing::warn!("checking enrichment jobs for {mbid} failed: {e}");
            (stored, EnrichmentStatus::Failed)
        }
    }
}

async fn status(state: &AppState, mbid: Uuid) -> anyhow::Result<EnrichmentStatus> {
    let max_age_secs = f64::from(MAX_AGE_DAYS) * 86_400.0;
    let queue = match jobs::latest(&state.pool, jobs::KIND_ENRICH, mbid).await? {
        Some((s, _)) if s == "pending" || s == "running" => false,
        Some((s, age)) if s == "done" && age < max_age_secs => {
            return Ok(EnrichmentStatus::Complete)
        }
        Some((s, age)) if s == "failed" && age < FAILED_RETRY_SECS => {
            return Ok(EnrichmentStatus::Failed)
        }
        Some(_) | None => true,
    };
    if queue {
        jobs::enqueue(&state.pool, jobs::KIND_ENRICH, mbid).await?;
    }
    Ok(EnrichmentStatus::Pending)
}

/// The worker's side: fetches from every source and stores what it finds. Fails if any source
/// failed, so the job is retried; sources that already succeeded are skipped on the retry.
pub async fn run_job(state: &AppState, mbid: Uuid) -> anyhow::Result<()> {
    let artist = get_artist(&state.mb_api_url, mbid, &state.cache).await?;
    let qid = artist
        .links
        .iter()
        .filter(|l| l.link_type == "wikidata")
        .find_map(|l| qid_from_url(&l.url));
    let (wikipedia, lastfm, fanart) = tokio::join!(
        ensure_wikipedia(state, mbid, qid),
        ensure_lastfm(state, mbid),
        ensure_fanart(state, mbid)
    );
    let mut errors: Vec<anyhow::Error> = [wikipedia, lastfm, fanart]
        .into_iter()
        .filter_map(Result::err)
        .collect();
    // A rate limit decides how the worker reacts, so report it ahead of other errors.
    errors.sort_by_key(|e| e.downcast_ref::<RateLimited>().is_none());
    match errors.into_iter().next() {
        Some(e) => Err(e),
        None => Ok(()),
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

async fn ensure_wikipedia(state: &AppState, mbid: Uuid, qid: Option<&str>) -> anyhow::Result<()> {
    let Some(qid) = qid else { return Ok(()) };
    let miss_key = format!("liner:enrich:miss:wikipedia:{mbid}");
    if bio_is_fresh(state, mbid, "wikipedia").await || state.cache.is_marked(&miss_key).await {
        return Ok(());
    }
    match state.wiki.summary_for_qid(qid).await? {
        Some(s) => {
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
            store(state, mbid, "wikipedia", Some(&bio), &images, None).await
        }
        None => {
            state.cache.mark(&miss_key, MISS_TTL).await;
            Ok(())
        }
    }
}

async fn ensure_lastfm(state: &AppState, mbid: Uuid) -> anyhow::Result<()> {
    let Some(lastfm) = &state.lastfm else {
        return Ok(());
    };
    let miss_key = format!("liner:enrich:miss:lastfm:{mbid}");
    if bio_is_fresh(state, mbid, "lastfm").await || state.cache.is_marked(&miss_key).await {
        return Ok(());
    }
    match lastfm.artist_info(mbid).await? {
        Some(info) => {
            let bio = (!info.summary.is_empty()).then(|| Bio {
                source: "lastfm".into(),
                lang: "en".into(),
                summary: info.summary,
                url: Some(info.url).filter(|u| !u.is_empty()),
                license: Some(lastfm::LICENSE.into()),
            });
            if bio.is_none() && info.tags.is_empty() {
                state.cache.mark(&miss_key, MISS_TTL).await;
                Ok(())
            } else {
                store(state, mbid, "lastfm", bio.as_ref(), &[], Some(&info.tags)).await
            }
        }
        None => {
            state.cache.mark(&miss_key, MISS_TTL).await;
            Ok(())
        }
    }
}

async fn ensure_fanart(state: &AppState, mbid: Uuid) -> anyhow::Result<()> {
    let Some(fanart) = &state.fanart else {
        return Ok(());
    };
    let done_key = format!("liner:enrich:done:fanart:{mbid}");
    if state.cache.is_marked(&done_key).await {
        return Ok(());
    }
    let images: Vec<Image> = fanart
        .artist_images(mbid)
        .await?
        .into_iter()
        .map(|i| Image {
            kind: i.kind.into(),
            url: i.url,
            source: "fanart".into(),
            width: None,
            height: None,
        })
        .collect();
    store(state, mbid, "fanart", None, &images, None).await?;
    state.cache.mark(&done_key, FANART_TTL).await;
    Ok(())
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
