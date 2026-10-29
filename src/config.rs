use std::env;

fn var(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub struct Config {
    pub port: u16,
    pub database_url: String,
    pub redis_url: String,
    pub musicbrainz_api_url: String,
    pub admin_token: Option<String>,
    pub trust_proxy: bool,
    pub rate_limit_anon_per_min: u32,
    pub rate_limit_key_per_min: u32,
    pub lastfm_api_key: Option<String>,
    pub fanart_api_key: Option<String>,
    pub llm_base_url: String,
    pub llm_api_key: Option<String>,
    pub llm_model: String,
    pub llm_daily_artist_cap: u32,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            port: parse("PORT", 3002),
            database_url: var("DATABASE_URL", "postgres://liner:liner@localhost:5433/liner"),
            redis_url: var("REDIS_URL", "redis://localhost:6380"),
            musicbrainz_api_url: var("MUSICBRAINZ_API_URL", "https://mb.halfnotes.club"),
            admin_token: env::var("ADMIN_TOKEN").ok(),
            trust_proxy: parse("TRUST_PROXY", true),
            rate_limit_anon_per_min: parse("RATE_LIMIT_ANON_PER_MIN", 60),
            rate_limit_key_per_min: parse("RATE_LIMIT_KEY_PER_MIN", 1200),
            lastfm_api_key: env::var("LASTFM_API_KEY").ok(),
            fanart_api_key: env::var("FANART_API_KEY").ok(),
            llm_base_url: var("LLM_BASE_URL", "https://api.openai.com/v1"),
            llm_api_key: env::var("LLM_API_KEY").ok(),
            llm_model: var("LLM_MODEL", "gpt-6.1-sol"),
            llm_daily_artist_cap: parse("LLM_DAILY_ARTIST_CAP", 200),
        }
    }
}
