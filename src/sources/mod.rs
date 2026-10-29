pub mod coverart;
pub mod fanart;
pub mod lastfm;
pub mod listenbrainz;
pub mod wikipedia;

pub const USER_AGENT: &str = "liner/0.1 (https://github.com/aglowinthefield/liner)";

pub fn http_client(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .expect("reqwest client")
}

/// An outside service answered 429. The worker pauses that kind of job until it may retry.
#[derive(Debug)]
pub struct RateLimited {
    pub source: &'static str,
    pub retry_after: Option<std::time::Duration>,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} rate limited us", self.source)?;
        if let Some(d) = self.retry_after {
            write!(f, ", retry after {}s", d.as_secs())?;
        }
        Ok(())
    }
}

impl std::error::Error for RateLimited {}

/// Sends a request, counts it in `liner_source_requests_total{source, outcome}`, and turns a
/// 429 into [`RateLimited`]. Other statuses are returned for the caller to handle.
pub async fn send(
    source: &'static str,
    request: reqwest::RequestBuilder,
) -> anyhow::Result<reqwest::Response> {
    let count = |outcome: &'static str| {
        metrics::counter!("liner_source_requests_total", "source" => source, "outcome" => outcome)
            .increment(1);
    };
    let resp = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            count("network_error");
            return Err(e.into());
        }
    };
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        count("rate_limited");
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(std::time::Duration::from_secs);
        return Err(RateLimited {
            source,
            retry_after,
        }
        .into());
    }
    count(if status == reqwest::StatusCode::NOT_FOUND {
        "not_found"
    } else if status.is_success() || status.is_redirection() {
        "ok"
    } else {
        "error"
    });
    Ok(resp)
}
