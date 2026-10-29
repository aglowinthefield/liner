//! Last.fm artist bios, tags, and similar artists, looked up by MusicBrainz ID.

use anyhow::Result;
use serde::Deserialize;
use uuid::Uuid;

pub const LICENSE: &str = "CC BY-SA 3.0";

#[derive(Clone)]
pub struct Lastfm {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtistInfo {
    pub url: String,
    /// Plain text, without Last.fm's trailing "Read more" link.
    pub summary: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimilarArtist {
    pub mbid: Uuid,
    pub name: String,
    /// 0.0 to 1.0.
    pub score: f64,
}

#[derive(Deserialize)]
struct InfoResponse {
    artist: Option<InfoArtist>,
}

#[derive(Deserialize)]
struct InfoArtist {
    url: Option<String>,
    bio: Option<Bio>,
    tags: Option<Tags>,
}

#[derive(Deserialize)]
struct Bio {
    #[serde(default)]
    summary: String,
}

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    tag: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

#[derive(Deserialize)]
struct SimilarResponse {
    similarartists: Option<SimilarArtists>,
}

#[derive(Deserialize)]
struct SimilarArtists {
    #[serde(default)]
    artist: Vec<SimilarEntry>,
}

#[derive(Deserialize)]
struct SimilarEntry {
    name: String,
    #[serde(default)]
    mbid: String,
    /// Last.fm sends this as a string, for example "0.78".
    #[serde(default, rename = "match")]
    score: serde_json::Value,
}

/// Removes HTML tags and everything from the first link on (Last.fm ends bios with a
/// "Read more on Last.fm" link), then decodes the few entities Last.fm uses.
pub fn clean_summary(html: &str) -> String {
    let cut = html.find("<a ").unwrap_or(html.len());
    let mut out = String::new();
    let mut in_tag = false;
    for c in html[..cut].chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

impl Lastfm {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            client: super::http_client(4),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }

    pub fn production(api_key: &str) -> Self {
        Self::new("https://ws.audioscrobbler.com", api_key)
    }

    async fn call(
        &self,
        method: &str,
        mbid: Uuid,
        extra: &[(&str, &str)],
    ) -> Result<Option<serde_json::Value>> {
        let request = self
            .client
            .get(format!("{}/2.0/", self.base_url))
            .query(&[
                ("method", method),
                ("api_key", &self.api_key),
                ("format", "json"),
                ("mbid", &mbid.to_string()),
            ])
            .query(extra);
        let resp = super::send("lastfm", request).await?;
        let status = resp.status();
        // Last.fm reports "artist not found" (error 6) as HTTP 404 or as a 200 with an error body.
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body: serde_json::Value = resp.error_for_status()?.json().await?;
        match body.get("error").and_then(|e| e.as_i64()) {
            Some(6) => Ok(None),
            Some(code) => anyhow::bail!(
                "last.fm error {code}: {}",
                body["message"].as_str().unwrap_or("")
            ),
            None => Ok(Some(body)),
        }
    }

    pub async fn artist_info(&self, mbid: Uuid) -> Result<Option<ArtistInfo>> {
        let Some(body) = self.call("artist.getinfo", mbid, &[]).await? else {
            return Ok(None);
        };
        let Some(artist) = serde_json::from_value::<InfoResponse>(body)?.artist else {
            return Ok(None);
        };
        Ok(Some(ArtistInfo {
            url: artist.url.unwrap_or_default(),
            summary: clean_summary(&artist.bio.map(|b| b.summary).unwrap_or_default()),
            tags: artist
                .tags
                .map(|t| t.tag.into_iter().map(|t| t.name).collect())
                .unwrap_or_default(),
        }))
    }

    pub async fn similar(&self, mbid: Uuid, limit: u32) -> Result<Vec<SimilarArtist>> {
        let Some(body) = self
            .call("artist.getsimilar", mbid, &[("limit", &limit.to_string())])
            .await?
        else {
            return Ok(vec![]);
        };
        let entries = serde_json::from_value::<SimilarResponse>(body)?
            .similarartists
            .map(|s| s.artist)
            .unwrap_or_default();
        Ok(entries
            .into_iter()
            .filter_map(|e| {
                let mbid = Uuid::parse_str(&e.mbid).ok()?;
                let score = match &e.score {
                    serde_json::Value::String(s) => s.parse().ok()?,
                    serde_json::Value::Number(n) => n.as_f64()?,
                    _ => 0.0,
                };
                Some(SimilarArtist {
                    mbid,
                    name: e.name,
                    score,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::clean_summary;

    #[test]
    fn strips_read_more_link_and_tags() {
        let html = "Nirvana was a <b>band</b> &amp; more. <a href=\"https://www.last.fm/music/Nirvana\">Read more on Last.fm</a>. User-contributed text";
        assert_eq!(clean_summary(html), "Nirvana was a band & more.");
    }
}
