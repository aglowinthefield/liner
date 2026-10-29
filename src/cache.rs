use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;

const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60); // 24 hours
const SEARCH_TTL: Duration = Duration::from_secs(60 * 60); // 1 hour

#[derive(Clone)]
pub struct Cache {
    conn: ConnectionManager,
}

impl Cache {
    pub async fn connect(redis_url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(redis_url)?;
        let conn = ConnectionManager::new(client).await?;
        Ok(Self { conn })
    }

    pub async fn get_or_fetch<T, F, Fut>(&self, key: &str, fetch: F) -> anyhow::Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        self.get_or_fetch_with_ttl(key, DEFAULT_TTL, fetch).await
    }

    pub async fn get_or_fetch_search<T, F, Fut>(&self, key: &str, fetch: F) -> anyhow::Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        self.get_or_fetch_with_ttl(key, SEARCH_TTL, fetch).await
    }

    async fn get_or_fetch_with_ttl<T, F, Fut>(
        &self,
        key: &str,
        ttl: Duration,
        fetch: F,
    ) -> anyhow::Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<T>>,
    {
        // Try cache first, fall through on any error
        if let Ok(Some(cached)) = self.get::<T>(key).await {
            tracing::debug!("cache hit: {key}");
            return Ok(cached);
        }

        let value = fetch().await?;

        // Best-effort cache write
        if let Err(e) = self.set(key, &value, ttl).await {
            tracing::warn!("cache write failed for {key}: {e}");
        }

        Ok(value)
    }

    async fn get<T: DeserializeOwned>(&self, key: &str) -> anyhow::Result<Option<T>> {
        let mut conn = self.conn.clone();
        let raw: Option<String> = conn.get(key).await?;
        match raw {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    async fn set<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) -> anyhow::Result<()> {
        let mut conn = self.conn.clone();
        let json = serde_json::to_string(value)?;
        conn.set_ex::<_, _, ()>(key, &json, ttl.as_secs()).await?;
        Ok(())
    }
}
