//! Artist bios and images from Wikipedia, found through the artist's Wikidata link in MusicBrainz.

use anyhow::Result;
use reqwest::Url;
use serde::Deserialize;

#[derive(Clone)]
pub struct Wikipedia {
    client: reqwest::Client,
    wikidata_url: String,
    wikipedia_url: String,
}

impl Default for Wikipedia {
    fn default() -> Self {
        Self::new("https://www.wikidata.org", "https://en.wikipedia.org")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub title: String,
    pub extract: String,
    pub url: String,
    pub thumbnail: Option<Picture>,
    pub original: Option<Picture>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Picture {
    pub source: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

#[derive(Deserialize)]
struct WbEntities {
    #[serde(default)]
    entities: std::collections::HashMap<String, WbEntity>,
}

#[derive(Deserialize)]
struct WbEntity {
    #[serde(default)]
    sitelinks: std::collections::HashMap<String, WbSitelink>,
}

#[derive(Deserialize)]
struct WbSitelink {
    title: String,
}

#[derive(Deserialize)]
struct RestSummary {
    #[serde(rename = "type")]
    kind: Option<String>,
    title: String,
    #[serde(default)]
    extract: String,
    content_urls: Option<ContentUrls>,
    thumbnail: Option<Picture>,
    originalimage: Option<Picture>,
}

#[derive(Deserialize)]
struct ContentUrls {
    desktop: Option<PageUrl>,
}

#[derive(Deserialize)]
struct PageUrl {
    page: String,
}

/// Extracts the Wikidata ID (`Q11649`) from a MusicBrainz `wikidata` link.
pub fn qid_from_url(url: &str) -> Option<&str> {
    let last = url.trim_end_matches('/').rsplit('/').next()?;
    let digits = last.strip_prefix('Q')?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(last)
}

impl Wikipedia {
    pub fn new(wikidata_url: &str, wikipedia_url: &str) -> Self {
        Self {
            client: super::http_client(3),
            wikidata_url: wikidata_url.trim_end_matches('/').to_string(),
            wikipedia_url: wikipedia_url.trim_end_matches('/').to_string(),
        }
    }

    /// Returns the English Wikipedia summary for a Wikidata item, or `None` if the item has
    /// no English article or the article is a disambiguation page.
    pub async fn summary_for_qid(&self, qid: &str) -> Result<Option<Summary>> {
        let request = self
            .client
            .get(format!("{}/w/api.php", self.wikidata_url))
            .query(&[
                ("action", "wbgetentities"),
                ("ids", qid),
                ("props", "sitelinks"),
                ("sitefilter", "enwiki"),
                ("format", "json"),
            ]);
        let entities: WbEntities = super::send("wikidata", request)
            .await?
            .error_for_status()?
            .json()
            .await?;

        let Some(title) = entities
            .entities
            .get(qid)
            .and_then(|e| e.sitelinks.get("enwiki"))
            .map(|s| s.title.clone())
        else {
            return Ok(None);
        };

        let mut url = Url::parse(&format!("{}/api/rest_v1/page/summary/", self.wikipedia_url))?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("wikipedia url cannot be a base"))?
            .pop_if_empty()
            .push(&title);

        let resp = super::send("wikipedia", self.client.get(url)).await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let page: RestSummary = resp.error_for_status()?.json().await?;

        if page.kind.as_deref() == Some("disambiguation") || page.extract.trim().is_empty() {
            return Ok(None);
        }
        let url = page
            .content_urls
            .and_then(|c| c.desktop)
            .map(|d| d.page)
            .unwrap_or_else(|| format!("{}/wiki/{}", self.wikipedia_url, title.replace(' ', "_")));

        Ok(Some(Summary {
            title: page.title,
            extract: page.extract,
            url,
            thumbnail: page.thumbnail,
            original: page.originalimage,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::qid_from_url;

    #[test]
    fn parses_wikidata_urls() {
        assert_eq!(
            qid_from_url("https://www.wikidata.org/wiki/Q11649"),
            Some("Q11649")
        );
        assert_eq!(
            qid_from_url("https://www.wikidata.org/wiki/Q11649/"),
            Some("Q11649")
        );
        assert_eq!(
            qid_from_url("https://www.wikidata.org/wiki/Special:Foo"),
            None
        );
        assert_eq!(qid_from_url("https://www.wikidata.org/wiki/Q"), None);
    }
}
