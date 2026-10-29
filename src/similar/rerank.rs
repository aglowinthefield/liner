//! Asks the LLM to pick and rank the real "similar artists" out of a list of candidates.
//!
//! The model only chooses among candidates we pass in. Anything else it returns is dropped,
//! so it cannot invent artists or MBIDs.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::{json, Value};
use uuid::Uuid;

use super::model::Merged;
use crate::{artist::model::get_artist, llm::Llm, AppState};

pub const PROMPT_VERSION: i32 = 1;
const CANDIDATES_SENT: usize = 40;
const EDGES_KEPT: usize = 20;
const REASON_MAX_CHARS: usize = 140;
pub const KINDS: [&str; 6] = [
    "members",
    "collaborators",
    "scene",
    "sound",
    "influence",
    "label",
];

const SYSTEM: &str = "You are a music librarian building a graph of related artists. \
You get one artist and a numbered list of candidate artists suggested by listening data. \
Pick up to 20 candidates that a fan of the artist would genuinely want to hear next, best first. \
Drop candidates that only appear because they are popular, and candidates with no real musical or personal connection. \
Use only candidates from the list, identified by their mbid. Never add artists that are not in the list. \
For each pick give a score from 0 to 1, a kind (members: shared band members or side projects; collaborators: they have worked together; \
scene: same place and time or label scene; sound: they sound alike; influence: one clearly shaped the other; label: same label roster), \
and a reason of one plain sentence, at most 140 characters, that states a fact. Do not use hype words.";

pub struct ArtistContext {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub albums: Vec<String>,
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "similar": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "mbid": {"type": "string"},
                        "score": {"type": "number"},
                        "kind": {"type": "string", "enum": KINDS},
                        "reason": {"type": "string"},
                    },
                    "required": ["mbid", "score", "kind", "reason"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["similar"],
        "additionalProperties": false,
    })
}

pub fn user_prompt(artist: &ArtistContext, candidates: &[Merged]) -> String {
    let mut out = format!("Artist: {}\n", artist.name);
    if !artist.description.is_empty() {
        out.push_str(&format!("About: {}\n", artist.description));
    }
    if !artist.tags.is_empty() {
        out.push_str(&format!("Tags: {}\n", artist.tags.join(", ")));
    }
    if !artist.albums.is_empty() {
        out.push_str(&format!("Albums: {}\n", artist.albums.join("; ")));
    }
    out.push_str("\nCandidates (mbid | name | sources | listening score):\n");
    for c in candidates {
        out.push_str(&format!(
            "{} | {} | {} | {:.2}\n",
            c.mbid,
            c.name,
            c.sources.join("+"),
            c.score
        ));
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub to: Uuid,
    pub name: String,
    pub score: f64,
    pub kind: String,
    pub reason: String,
    pub sources: Vec<String>,
}

/// Keeps only well-formed picks of known candidates, best first.
pub fn validate(reply: &Value, candidates: &[Merged]) -> Vec<Edge> {
    let known: HashMap<Uuid, &Merged> = candidates.iter().map(|c| (c.mbid, c)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut edges = Vec::new();
    for item in reply["similar"].as_array().into_iter().flatten() {
        let Some(to) = item["mbid"].as_str().and_then(|s| Uuid::parse_str(s).ok()) else {
            continue;
        };
        let Some(candidate) = known.get(&to) else {
            continue;
        };
        let Some(score) = item["score"].as_f64().filter(|s| s.is_finite()) else {
            continue;
        };
        if !seen.insert(to) {
            continue;
        }
        let kind = item["kind"]
            .as_str()
            .filter(|k| KINDS.contains(k))
            .unwrap_or("sound")
            .to_string();
        let reason: String = item["reason"]
            .as_str()
            .unwrap_or("")
            .trim()
            .chars()
            .take(REASON_MAX_CHARS)
            .collect();
        edges.push(Edge {
            to,
            name: candidate.name.clone(),
            score: score.clamp(0.0, 1.0),
            kind,
            reason,
            sources: candidate.sources.clone(),
        });
    }
    edges.sort_by(|a, b| b.score.total_cmp(&a.score));
    edges.truncate(EDGES_KEPT);
    edges
}

async fn artist_context(state: &AppState, mbid: Uuid) -> Result<ArtistContext> {
    let artist = get_artist(&state.mb_api_url, mbid, &state.cache).await?;
    let tags: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM artist_tags WHERE mbid = $1 ORDER BY position LIMIT 8",
    )
    .bind(mbid)
    .fetch_all(&state.pool)
    .await?;
    let mut description: Vec<String> = Vec::new();
    description.extend(artist.artist_type.clone());
    description.extend(artist.country.clone());
    if let Some(begin) = &artist.begin_date {
        description.push(format!("active from {begin}"));
    }
    description.extend(artist.disambiguation.clone());
    Ok(ArtistContext {
        name: artist.name,
        description: description.join(", "),
        tags,
        albums: artist
            .release_groups
            .iter()
            .take(6)
            .map(|r| r.title.clone())
            .collect(),
    })
}

pub async fn rerank(
    state: &AppState,
    llm: &Llm,
    mbid: Uuid,
    mut candidates: Vec<Merged>,
) -> Result<()> {
    candidates.truncate(CANDIDATES_SENT);
    let artist = artist_context(state, mbid).await?;
    let reply = llm
        .complete_json(
            SYSTEM,
            &user_prompt(&artist, &candidates),
            "similar_artists",
            schema(),
        )
        .await?;
    let edges = validate(&reply, &candidates);
    if edges.is_empty() {
        anyhow::bail!("llm returned no usable picks");
    }

    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM similar_edges WHERE from_mbid = $1")
        .bind(mbid)
        .execute(&mut *tx)
        .await?;
    for e in &edges {
        sqlx::query(
            "INSERT INTO similar_edges (from_mbid, to_mbid, to_name, score, kind, reason, sources, model, prompt_version)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(mbid)
        .bind(e.to)
        .bind(&e.name)
        .bind(e.score)
        .bind(&e.kind)
        .bind(&e.reason)
        .bind(&e.sources)
        .bind(&llm.model)
        .bind(PROMPT_VERSION)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(n: u128) -> Merged {
        Merged {
            mbid: Uuid::from_u128(n),
            name: format!("Artist {n}"),
            score: 0.5,
            sources: vec!["lastfm".into()],
        }
    }

    #[test]
    fn drops_invented_and_malformed_picks() {
        let candidates = vec![cand(1), cand(2)];
        let reply = json!({"similar": [
            {"mbid": Uuid::from_u128(1).to_string(), "score": 1.7, "kind": "sound", "reason": "  Same scene.  "},
            {"mbid": Uuid::from_u128(99).to_string(), "score": 0.9, "kind": "sound", "reason": "invented"},
            {"mbid": "not-a-uuid", "score": 0.9, "kind": "sound", "reason": "bad id"},
            {"mbid": Uuid::from_u128(1).to_string(), "score": 0.2, "kind": "sound", "reason": "duplicate"},
            {"mbid": Uuid::from_u128(2).to_string(), "score": 0.4, "kind": "vibes", "reason": "x".repeat(300)},
        ]});
        let edges = validate(&reply, &candidates);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].to, Uuid::from_u128(1));
        assert_eq!(edges[0].score, 1.0);
        assert_eq!(edges[0].reason, "Same scene.");
        assert_eq!(edges[1].kind, "sound");
        assert_eq!(edges[1].reason.chars().count(), 140);
    }
}
