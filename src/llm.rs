//! Client for an OpenAI-compatible chat completions API that returns JSON matching a schema.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::cache::Cache;

#[derive(Clone)]
pub struct Llm {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    pub model: String,
    daily_cap: u32,
}

impl Llm {
    pub fn new(base_url: &str, api_key: &str, model: &str, daily_cap: u32) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
            daily_cap,
        }
    }

    /// Counts one artist against today's cap. Returns false once the cap is used up.
    pub async fn take_daily_slot(&self, cache: &Cache) -> bool {
        let key = format!("liner:llm:day:{}", chrono::Utc::now().format("%Y-%m-%d"));
        match cache
            .incr_window(&key, Duration::from_secs(26 * 60 * 60))
            .await
        {
            Ok(n) => n <= u64::from(self.daily_cap),
            // Without Redis we can't count, so refuse rather than risk unbounded spend.
            Err(e) => {
                tracing::warn!("llm daily cap counter failed: {e}");
                false
            }
        }
    }

    /// Sends a system and user message and returns the reply parsed as JSON matching `schema`.
    pub async fn complete_json(
        &self,
        system: &str,
        user: &str,
        schema_name: &str,
        schema: Value,
    ) -> Result<Value> {
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": schema_name, "strict": true, "schema": schema},
            },
        });
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "llm returned {status}: {}",
                text.chars().take(300).collect::<String>()
            );
        }
        let reply: Value = resp.json().await?;
        let content = reply["choices"][0]["message"]["content"]
            .as_str()
            .context("llm reply has no content")?;
        serde_json::from_str(content).context("llm reply is not valid JSON")
    }
}
