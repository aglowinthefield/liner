use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::rerank;
use crate::{jobs, AppState};

const CANDIDATES_MAX_AGE_DAYS: i32 = 7;
const REQUEST_RETRY: std::time::Duration = std::time::Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SimilarStatus {
    /// Ranked by the LLM, with a `kind` and `reason` for each artist.
    Ready,
    /// Merged from ListenBrainz and Last.fm, not ranked by the LLM.
    Candidates,
    /// Nothing yet. A job is running, so ask again in a few seconds.
    Pending,
}

#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
pub struct SimilarArtist {
    pub mbid: Uuid,
    pub name: String,
    /// 0.0 to 1.0, highest first.
    pub score: f64,
    /// How the artists relate: `members`, `collaborators`, `scene`, `sound`, `influence`, or `label`.
    /// Only set when `status` is `ready`.
    pub kind: Option<String>,
    /// One sentence on why they are similar. Only set when `status` is `ready`.
    pub reason: Option<String>,
    /// Where the pair showed up: `listenbrainz`, `lastfm`.
    pub sources: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SimilarResponse {
    pub mbid: Uuid,
    pub name: String,
    pub status: SimilarStatus,
    pub similar: Vec<SimilarArtist>,
}

/// One similar artist merged across sources.
#[derive(Debug, Clone, PartialEq)]
pub struct Merged {
    pub mbid: Uuid,
    pub name: String,
    pub score: f64,
    pub sources: Vec<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct CandidateRow {
    to_mbid: Uuid,
    to_name: String,
    source: String,
    raw_score: f64,
}

/// Merges per-source candidates into one list, best first.
///
/// Each source's scores are scaled so its best candidate is 1.0, then a candidate's score is the
/// average over every source that returned anything, so artists that several sources agree on rank higher.
fn merge(rows: Vec<CandidateRow>) -> Vec<Merged> {
    let mut max_by_source: HashMap<&str, f64> = HashMap::new();
    for r in &rows {
        let m = max_by_source.entry(r.source.as_str()).or_insert(0.0);
        *m = m.max(r.raw_score);
    }
    let source_count = max_by_source.len().max(1) as f64;
    let mut by_mbid: HashMap<Uuid, Merged> = HashMap::new();
    for r in &rows {
        let max = max_by_source[r.source.as_str()];
        let scaled = if max > 0.0 { r.raw_score / max } else { 0.0 };
        let entry = by_mbid.entry(r.to_mbid).or_insert_with(|| Merged {
            mbid: r.to_mbid,
            name: r.to_name.clone(),
            score: 0.0,
            sources: vec![],
        });
        entry.score += scaled / source_count;
        entry.sources.push(r.source.clone());
        if entry.name.is_empty() {
            entry.name = r.to_name.clone();
        }
    }
    let mut merged: Vec<Merged> = by_mbid.into_values().collect();
    for m in &mut merged {
        m.sources.sort();
    }
    merged.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.mbid.cmp(&b.mbid)));
    merged
}

pub async fn load_candidates(state: &AppState, mbid: Uuid) -> Result<Vec<Merged>> {
    let rows: Vec<CandidateRow> = sqlx::query_as(
        "SELECT to_mbid, to_name, source, raw_score FROM similar_candidates WHERE from_mbid = $1",
    )
    .bind(mbid)
    .fetch_all(&state.pool)
    .await?;
    Ok(merge(rows))
}

pub async fn load_edges(state: &AppState, mbid: Uuid, limit: i64) -> Result<Vec<SimilarArtist>> {
    Ok(sqlx::query_as(
        "SELECT to_mbid AS mbid, to_name AS name, score, kind, reason, sources
         FROM similar_edges WHERE from_mbid = $1 ORDER BY score DESC, to_mbid LIMIT $2",
    )
    .bind(mbid)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?)
}

/// Returns the best data stored for the artist and queues a job if something better could be made.
pub async fn lookup(
    state: &AppState,
    mbid: Uuid,
    limit: usize,
) -> Result<(SimilarStatus, Vec<SimilarArtist>)> {
    let edges = load_edges(state, mbid, limit as i64).await?;
    if !edges.is_empty() {
        return Ok((SimilarStatus::Ready, edges));
    }

    let candidates = load_candidates(state, mbid).await?;
    // Ask for a job when there is nothing yet, or when the LLM could still rank what we have.
    if candidates.is_empty() || state.llm.is_some() {
        request(state, mbid).await;
    }
    if !candidates.is_empty() {
        let similar = candidates
            .into_iter()
            .take(limit)
            .map(|c| SimilarArtist {
                mbid: c.mbid,
                name: c.name,
                score: c.score,
                kind: None,
                reason: None,
                sources: c.sources,
            })
            .collect();
        return Ok((SimilarStatus::Candidates, similar));
    }

    // No candidates. If a job already finished, there are none to find.
    let status = match jobs::latest_status(&state.pool, jobs::KIND_SIMILAR, mbid).await? {
        Some(s) if s == "done" || s == "failed" => SimilarStatus::Candidates,
        _ => SimilarStatus::Pending,
    };
    Ok((status, vec![]))
}

/// Queues a similar-artists job, at most once an hour per artist.
pub async fn request(state: &AppState, mbid: Uuid) {
    let key = format!("liner:similar:requested:{mbid}");
    if state.cache.is_marked(&key).await {
        return;
    }
    match jobs::enqueue(&state.pool, jobs::KIND_SIMILAR, mbid).await {
        Ok(_) => state.cache.mark(&key, REQUEST_RETRY).await,
        Err(e) => tracing::warn!("queueing similar job for {mbid} failed: {e}"),
    }
}

/// Collects candidates if ours are old, then has the LLM rank them.
pub async fn run_job(state: &AppState, mbid: Uuid) -> Result<()> {
    let fresh: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM similar_candidates WHERE from_mbid = $1 AND fetched_at > now() - make_interval(days => $2) LIMIT 1",
    )
    .bind(mbid)
    .bind(CANDIDATES_MAX_AGE_DAYS)
    .fetch_optional(&state.pool)
    .await?;
    if fresh.is_none() {
        collect_candidates(state, mbid).await?;
    }

    let Some(llm) = &state.llm else { return Ok(()) };
    let candidates = load_candidates(state, mbid).await?;
    if candidates.is_empty() {
        return Ok(());
    }
    if !llm.take_daily_slot(&state.cache).await {
        tracing::info!("llm daily cap reached, leaving {mbid} as candidates");
        return Ok(());
    }
    rerank::rerank(state, llm, mbid, candidates).await
}

/// A similar artist as one source reported it: MBID, name, raw score.
type Found = (Uuid, String, f64);

async fn collect_candidates(state: &AppState, mbid: Uuid) -> Result<()> {
    let lastfm = async {
        match &state.lastfm {
            Some(l) => Some(l.similar(mbid, 50).await),
            None => None,
        }
    };
    let (lb, lf) = tokio::join!(state.listenbrainz.similar(mbid), lastfm);

    let mut sets: Vec<(&str, Vec<Found>)> = Vec::new();
    let mut errors = Vec::new();
    match lb {
        Ok(found) => sets.push((
            "listenbrainz",
            found
                .into_iter()
                .map(|a| (a.artist_mbid, a.name, a.score))
                .collect(),
        )),
        Err(e) => errors.push(format!("listenbrainz: {e}")),
    }
    match lf {
        Some(Ok(found)) => sets.push((
            "lastfm",
            found
                .into_iter()
                .map(|a| (a.mbid, a.name, a.score))
                .collect(),
        )),
        Some(Err(e)) => errors.push(format!("lastfm: {e}")),
        None => {}
    }
    // If every source failed, retry the job instead of recording "no candidates".
    if sets.is_empty() {
        anyhow::bail!("no similar-artist source answered: {}", errors.join("; "));
    }
    for e in &errors {
        tracing::warn!("similar candidates for {mbid}: {e}");
    }

    let mut tx = state.pool.begin().await?;
    for (source, found) in sets {
        sqlx::query("DELETE FROM similar_candidates WHERE from_mbid = $1 AND source = $2")
            .bind(mbid)
            .bind(source)
            .execute(&mut *tx)
            .await?;
        for (to, name, score) in found.into_iter().filter(|(to, _, _)| *to != mbid) {
            sqlx::query(
                "INSERT INTO similar_candidates (from_mbid, to_mbid, source, raw_score, to_name)
                 VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
            )
            .bind(mbid)
            .bind(to)
            .bind(source)
            .bind(score)
            .bind(name)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(to: u128, source: &str, score: f64) -> CandidateRow {
        CandidateRow {
            to_mbid: Uuid::from_u128(to),
            to_name: format!("a{to}"),
            source: source.into(),
            raw_score: score,
        }
    }

    #[test]
    fn agreement_between_sources_ranks_higher() {
        let merged = merge(vec![
            row(1, "listenbrainz", 1000.0),
            row(2, "listenbrainz", 500.0),
            row(2, "lastfm", 0.9),
            row(3, "lastfm", 1.0),
        ]);
        let order: Vec<u128> = merged.iter().map(|m| m.mbid.as_u128()).collect();
        // 2: (0.5 + 0.9) / 2 = 0.70, 1: 0.5, 3: 0.5
        assert_eq!(order[0], 2);
        assert_eq!(merged[0].sources, vec!["lastfm", "listenbrainz"]);
        assert!((merged[0].score - 0.7).abs() < 1e-9);
    }

    #[test]
    fn a_single_source_scores_up_to_one() {
        let merged = merge(vec![
            row(1, "listenbrainz", 40.0),
            row(2, "listenbrainz", 20.0),
        ]);
        assert!((merged[0].score - 1.0).abs() < 1e-9);
        assert!((merged[1].score - 0.5).abs() < 1e-9);
    }
}
