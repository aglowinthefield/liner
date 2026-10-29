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
