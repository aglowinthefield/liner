use anyhow::Result;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::cache::Cache;

#[derive(Debug, Deserialize, ToSchema)]
pub struct MatchRequest {
    pub albums: Vec<AlbumToMatch>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AlbumToMatch {
    pub artist_name: String,
    pub album_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MatchResponse {
    pub matches: Vec<AlbumMatch>,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct AlbumMatch {
    pub artist_name: String,
    pub album_name: String,
    pub status: MatchStatus,
    /// Release group MBID of the best candidate. Only set when `status` is `confident`.
    pub release_group_mbid: Option<String>,
    /// Artist MBID of the best candidate. Set when `status` is `confident`, and also when the
    /// album is only a near match but the top candidate's artist name is the one asked for.
    pub artist_mbid: Option<Uuid>,
    pub candidates: Vec<MatchCandidate>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchStatus {
    Confident,
    Ambiguous,
    NotFound,
}

#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct MatchCandidate {
    pub mbid: String,
    pub title: String,
    pub artist_name: String,
    /// MBID of the first credited artist. Use it with `/v1/artist/{mbid}`.
    #[serde(default)]
    pub artist_mbid: Option<Uuid>,
    pub release_group_type: Option<String>,
    pub first_release_date: Option<String>,
    pub score: u8,
}

// --- MB search response shapes ---

#[derive(Debug, Deserialize)]
struct MbReleaseGroupSearchResponse {
    #[serde(rename = "release-groups", default)]
    release_groups: Vec<MbReleaseGroupHit>,
}

#[derive(Debug, Deserialize)]
struct MbReleaseGroupHit {
    id: Uuid,
    title: String,
    score: u8,
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(rename = "first-release-date")]
    first_release_date: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<MbArtistCredit>,
}

#[derive(Debug, Deserialize)]
struct MbArtistCredit {
    name: String,
    #[serde(default)]
    joinphrase: Option<String>,
    artist: Option<MbCreditedArtist>,
}

#[derive(Debug, Deserialize)]
struct MbCreditedArtist {
    id: Uuid,
}

/// Lowercases and flattens a title or name for comparison: unicode punctuation becomes ASCII,
/// invisible characters and anything but letters and digits are dropped, and words are
/// separated by single spaces.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = true;
    for c in s.chars() {
        let c = match c {
            '\u{2018}' | '\u{2019}' | '\u{2032}' => '\'',
            _ => c,
        };
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            gap = false;
        } else if c == '\'' || is_invisible(c) {
            // "don't" and "dont" should compare equal; invisible characters vanish.
        } else if !gap {
            out.push(' ');
            gap = true;
        }
    }
    out.trim_end().to_string()
}

fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}' | '\u{feff}')
}

/// Drops trailing "(...)" and "[...]" groups such as "(Remastered)" or "(A ver.)", and a
/// trailing " Soundtrack", so the plain title can be searched and compared. Returns the
/// input unchanged if nothing would be left.
pub fn strip_edition(title: &str) -> String {
    let mut t = title.trim().to_string();
    loop {
        let trimmed = t.trim_end();
        let close = match trimmed.chars().last() {
            Some(')') => '(',
            Some(']') => '[',
            _ => break,
        };
        match trimmed.rfind(close) {
            Some(i) if i > 0 => t = trimmed[..i].trim_end().to_string(),
            _ => break,
        }
    }
    let lower = t.to_lowercase();
    if let Some(stem) = lower.strip_suffix(" soundtrack") {
        t.truncate(stem.len());
    }
    if t.trim().is_empty() {
        title.trim().to_string()
    } else {
        t.trim().to_string()
    }
}

fn artist_matches(candidates: &[MatchCandidate], artist_name: &str) -> bool {
    candidates.first().is_some_and(|c| {
        normalize(&c.artist_name) == normalize(artist_name) && c.artist_mbid.is_some()
    })
}

#[derive(Debug, Deserialize)]
struct MbArtistSearch {
    #[serde(default)]
    artists: Vec<MbArtistHit>,
}

#[derive(Debug, Deserialize)]
struct MbArtistHit {
    id: Uuid,
    name: String,
    score: u8,
}

/// The artist whose name is exactly `artist_name` (ignoring case and punctuation), if there is
/// exactly one such artist at the top of MusicBrainz's search. Two artists with the same name
/// are a coin flip, so those return None.
async fn artist_by_name(base_url: &str, cache: &Cache, artist_name: &str) -> Option<Uuid> {
    let wanted = normalize(artist_name);
    if wanted.is_empty() {
        return None;
    }
    let key = format!("liner:match:artist:v1:{wanted}");
    let hits: Vec<(Uuid, String, u8)> = cache
        .get_or_fetch(&key, || async {
            let resp = reqwest::Client::new()
                .get(format!("{base_url}/ws/2/artist"))
                .query(&[
                    (
                        "query",
                        format!("artist:\"{}\"", clean_title(artist_name).replace('"', ""))
                            .as_str(),
                    ),
                    ("fmt", "json"),
                    ("limit", "5"),
                ])
                .send()
                .await?
                .error_for_status()?;
            let body: MbArtistSearch = resp.json().await?;
            Ok(body
                .artists
                .into_iter()
                .map(|a| (a.id, a.name, a.score))
                .collect())
        })
        .await
        .unwrap_or_default();
    let mut exact = hits
        .iter()
        .filter(|(_, name, score)| *score >= 90 && normalize(name) == wanted);
    let first = exact.next()?;
    exact.next().is_none().then_some(first.0)
}

/// The first artist of a joined credit such as "vai5000， maji*", when the tag has separators
/// that appear in multi-artist credits. A single-artist name returns None.
fn lead_artist(artist_name: &str) -> Option<&str> {
    let (lead, _) = artist_name.split_once(['，', '、', ';', '；'])?;
    let lead = lead.trim();
    (!lead.is_empty()).then_some(lead)
}

fn clean_title(title: &str) -> String {
    title.chars().filter(|c| !is_invisible(*c)).collect()
}

/// Search MusicBrainz for a single (artist, album) pair and return ranked candidates.
pub async fn match_album(
    base_url: &str,
    cache: &Cache,
    artist_name: &str,
    album_name: &str,
) -> AlbumMatch {
    let cache_key = format!(
        "liner:match:v3:{}:{}",
        artist_name.to_lowercase().trim(),
        album_name.to_lowercase().trim()
    );

    let candidates = cache
        .get_or_fetch(&cache_key, || {
            fetch_candidates(base_url, artist_name, album_name)
        })
        .await
        .unwrap_or_default();

    let wanted = normalize(album_name);
    let wanted_plain = normalize(&strip_edition(album_name));
    let title_matches = |title: &str| {
        let n = normalize(title);
        n == wanted || normalize(&strip_edition(title)) == wanted_plain
    };
    let status = if candidates.is_empty() {
        MatchStatus::NotFound
    } else if candidates[0].score >= 90 && title_matches(&candidates[0].title) {
        MatchStatus::Confident
    } else if candidates[0].score >= 70 {
        MatchStatus::Ambiguous
    } else {
        MatchStatus::NotFound
    };

    let best = candidates
        .first()
        .filter(|_| status == MatchStatus::Confident);
    // The album search misses artists credited differently on their records, and albums
    // MusicBrainz lacks. If it found nobody, look for the artist by name alone.
    let by_name = if best.is_none() && !artist_matches(&candidates, artist_name) {
        match artist_by_name(base_url, cache, artist_name).await {
            Some(id) => Some(id),
            None => match lead_artist(artist_name) {
                Some(lead) => artist_by_name(base_url, cache, lead).await,
                None => None,
            },
        }
    } else {
        None
    };
    // Even when the album is not pinned down, a top hit by an artist with exactly this name
    // identifies the artist. Compare against the full credit so "Lyle & Peake" is not "Peake".
    let artist = candidates
        .first()
        .filter(|c| {
            status != MatchStatus::NotFound && normalize(&c.artist_name) == normalize(artist_name)
        })
        .and_then(|c| c.artist_mbid)
        .or(by_name);
    AlbumMatch {
        artist_name: artist_name.to_string(),
        album_name: album_name.to_string(),
        release_group_mbid: best.map(|c| c.mbid.clone()),
        artist_mbid: best.and_then(|c| c.artist_mbid).or(artist),
        status,
        candidates,
    }
}

async fn fetch_candidates(
    base_url: &str,
    artist_name: &str,
    album_name: &str,
) -> Result<Vec<MatchCandidate>> {
    let query = format!(
        "\"{}\" AND artist:\"{}\"",
        clean_title(&strip_edition(album_name)).replace('"', ""),
        clean_title(artist_name).replace('"', "")
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base_url}/ws/2/release-group"))
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
        .send()
        .await?;

    let body: MbReleaseGroupSearchResponse = resp.json().await?;

    let candidates: Vec<MatchCandidate> = body
        .release_groups
        .into_iter()
        .take(3)
        .map(|rg| {
            let artist_mbid = rg
                .artist_credit
                .first()
                .and_then(|ac| ac.artist.as_ref())
                .map(|a| a.id);
            let artist = rg
                .artist_credit
                .into_iter()
                .map(|ac| {
                    let join = ac.joinphrase.unwrap_or_default();
                    format!("{}{}", ac.name, join)
                })
                .collect::<String>();

            MatchCandidate {
                mbid: rg.id.to_string(),
                title: rg.title,
                artist_name: artist,
                artist_mbid,
                release_group_type: rg.primary_type,
                first_release_date: rg.first_release_date.filter(|d| !d.is_empty()),
                score: rg.score,
            }
        })
        .collect();

    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_and_case_compare_equal() {
        assert_eq!(
            normalize("Destination... Out!"),
            normalize("Destination\u{2026} Out!")
        );
        assert_eq!(normalize("Don\u{2019}t Stop"), normalize("dont stop"));
        assert_eq!(normalize("\u{200e}waifu"), "waifu");
        assert_eq!(normalize("Lyle & Peake"), "lyle peake");
    }

    #[test]
    fn joined_credits_lead_with_the_first_artist() {
        assert_eq!(lead_artist("vai5000， maji*"), Some("vai5000"));
        assert_eq!(lead_artist("a; b"), Some("a"));
        assert_eq!(lead_artist("Tyler, the Creator"), None);
        assert_eq!(lead_artist("Opal"), None);
    }

    #[test]
    fn edition_tags_are_stripped() {
        assert_eq!(
            strip_edition("Acid Angel from Asia <ACCESS> (A ver.)"),
            "Acid Angel from Asia <ACCESS>"
        );
        assert_eq!(
            strip_edition("Blues (original stereo release) [2011 remaster]"),
            "Blues"
        );
        assert_eq!(strip_edition("Diablo II Soundtrack"), "Diablo II");
        assert_eq!(strip_edition("(Untitled)"), "(Untitled)");
        assert_eq!(strip_edition("Plain"), "Plain");
    }
}
