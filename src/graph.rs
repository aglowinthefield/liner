//! A walk over the stored similar-artist edges, for drawing a graph around one artist.

use std::collections::{BTreeMap, HashSet};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{artist::model::get_artist, musicbrainz, similar::model as similar, AppState};

const MAX_DEPTH: u32 = 3;

#[derive(Debug, Deserialize, IntoParams)]
pub struct GraphParams {
    /// How many hops to walk from the artist, 1 to 3. Default 1.
    pub depth: Option<u32>,
    /// Most edges to follow out of each artist, 1 to 20. Default 8.
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GraphNode {
    pub mbid: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct GraphEdge {
    pub from: Uuid,
    pub to: Uuid,
    pub score: f64,
    pub kind: String,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Graph {
    pub root: Uuid,
    /// `pending` while the root artist has no ranked edges yet; ask again in a few seconds.
    pub status: similar::SimilarStatus,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, sqlx::FromRow)]
struct EdgeRow {
    from_mbid: Uuid,
    to_mbid: Uuid,
    to_name: String,
    score: f64,
    kind: String,
    reason: Option<String>,
}

#[utoipa::path(
    get, path = "/v1/graph/{mbid}",
    params(("mbid" = Uuid, Path, description = "MusicBrainz artist ID"), GraphParams),
    responses(
        (status = 200, description = "Nodes and edges around the artist", body = Graph),
        (status = 404, description = "No such artist in MusicBrainz"),
        (status = 500, description = "Failed to build the graph"),
    ),
    tag = "artist"
)]
pub async fn get_graph(
    State(state): State<AppState>,
    Path(mbid): Path<Uuid>,
    Query(params): Query<GraphParams>,
) -> impl IntoResponse {
    let artist = match get_artist(&state.mb_api_url, mbid, &state.cache).await {
        Ok(a) => a,
        Err(e) => return musicbrainz::error_response(&e, "artist"),
    };
    let depth = params.depth.unwrap_or(1).clamp(1, MAX_DEPTH);
    let limit = params.limit.unwrap_or(8).clamp(1, 20);
    match build(&state, mbid, artist.name, depth, limit).await {
        Ok(graph) => (StatusCode::OK, Json(graph)).into_response(),
        Err(e) => {
            tracing::error!("graph for {mbid} failed: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed to build the graph" })),
            )
                .into_response()
        }
    }
}

async fn build(
    state: &AppState,
    root: Uuid,
    root_name: String,
    depth: u32,
    limit: i64,
) -> anyhow::Result<Graph> {
    let mut nodes: BTreeMap<Uuid, String> = BTreeMap::from([(root, root_name)]);
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut visited: HashSet<Uuid> = HashSet::new();
    let mut frontier = vec![root];

    for _ in 0..depth {
        frontier.retain(|m| visited.insert(*m));
        if frontier.is_empty() {
            break;
        }
        let rows: Vec<EdgeRow> = sqlx::query_as(
            "SELECT from_mbid, to_mbid, to_name, score, kind, reason FROM (
                 SELECT *, row_number() OVER (PARTITION BY from_mbid ORDER BY score DESC, to_mbid) AS rn
                 FROM similar_edges WHERE from_mbid = ANY($1)
             ) ranked WHERE rn <= $2 ORDER BY from_mbid, rn",
        )
        .bind(&frontier)
        .bind(limit)
        .fetch_all(&state.pool)
        .await?;

        let mut next = Vec::new();
        for r in rows {
            nodes.entry(r.to_mbid).or_insert(r.to_name);
            next.push(r.to_mbid);
            edges.push(GraphEdge {
                from: r.from_mbid,
                to: r.to_mbid,
                score: r.score,
                kind: r.kind,
                reason: r.reason,
            });
        }
        frontier = next;
    }

    let status = if edges.iter().any(|e| e.from == root) {
        similar::SimilarStatus::Ready
    } else {
        // The root has no ranked edges. Start a job, and report whether one is on the way.
        similar::request(state, root).await;
        let has_candidates = !similar::load_candidates(state, root).await?.is_empty();
        match crate::jobs::latest_status(&state.pool, crate::jobs::KIND_SIMILAR, root)
            .await?
            .as_deref()
        {
            Some("done") | Some("failed") => similar::SimilarStatus::Candidates,
            _ if has_candidates => similar::SimilarStatus::Candidates,
            _ => similar::SimilarStatus::Pending,
        }
    };

    Ok(Graph {
        root,
        status,
        nodes: nodes
            .into_iter()
            .map(|(mbid, name)| GraphNode { mbid, name })
            .collect(),
        edges,
    })
}
