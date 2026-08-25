use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Simple Tokenomist wrapper with in-memory TTL cache and retry/backoff.
///
/// Usage:
/// let client = TokenomistClient::new(Duration::from_secs(300));
/// let plans = client.fetch_unlock_schedule("https://tokenomist.example/api/unlocks").await?;
#[derive(Debug, Clone, Deserialize)]
pub struct UnlockPlan {
    pub token: String,
    pub amount: f64,
    pub timestamp: u64,
}

pub struct TokenomistClient {
    client: reqwest::Client,
    cache_ttl: Duration,
    cache: RwLock<Option<(Instant, Vec<UnlockPlan>)>>,
}

impl TokenomistClient {
    pub fn new(cache_ttl: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .use_rustls_tls()
            .build()
            .expect("failed to build reqwest client");

        Self {
            client,
            cache_ttl,
            cache: RwLock::new(None),
        }
    }

    /// Fetch unlock schedule from Tokenomist-like endpoint.
    /// Implements exponential backoff with jitter and caches results for cache_ttl.
    pub async fn fetch_unlock_schedule(
        &self,
        api_url: &str,
    ) -> Result<Vec<UnlockPlan>, Box<dyn std::error::Error + Send + Sync>> {
        // check cache
        {
            let guard = self.cache.read().await;
            if let Some((ts, ref v)) = &*guard {
                if ts.elapsed() < self.cache_ttl {
                    return Ok(v.clone());
                }
            }
        }

        // Retry loop
        let mut attempt = 0u32;
        let max_attempts = 5u32;
        let mut backoff = 1u64; // seconds

        loop {
            attempt += 1;
            let resp = self.client.get(api_url).send().await;
            match resp {
                Ok(r) => {
                    if r.status().is_success() {
                        let parsed: Result<Vec<UnlockPlan>, _> = r.json().await;
                        match parsed {
                            Ok(plans) => {
                                // store in cache
                                let mut guard = self.cache.write().await;
                                *guard = Some((Instant::now(), plans.clone()));
                                return Ok(plans);
                            }
                            Err(e) => {
                                // JSON parse error
                                if attempt >= max_attempts {
                                    return Err(format!("json parse error: {}", e).into());
                                }
                                // fallthrough to backoff
                            }
                        }
                    } else if r.status().as_u16() == 429 {
                        // rate limited -> backoff
                        if attempt >= max_attempts {
                            return Err(format!("rate limited: {}", r.status()).into());
                        }
                    } else if r.status().is_server_error() {
                        if attempt >= max_attempts {
                            return Err(format!("server error: {}", r.status()).into());
                        }
                    } else {
                        // unexpected client error; don't retry
                        return Err(format!("unexpected status: {}", r.status()).into());
                    }
                }
                Err(e) => {
                    if attempt >= max_attempts {
                        return Err(e.into());
                    }
                    // else fallthrough to retry
                }
            }

            // backoff with jitter
            let jitter = rand::random::<u64>() % 300; // up to 300ms jitter
            let wait_ms = (backoff * 1000) + jitter;
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            backoff = (backoff * 2).min(32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[tokio::test]
    async fn test_fetch_unlock_schedule_caches() {
        let server = MockServer::start_async().await;
        let body = r#"[{"token":"HYPE","amount":12345.0,"timestamp":1700000000}]"#;
        let m = server.mock(|when, then| {
            when.method(GET).path("/unlocks");
            then.status(200).header("content-type", "application/json").body(body);
        });

        let client = TokenomistClient::new(Duration::from_secs(60));
        let url = format!("{}/unlocks", server.base_url());

        let first = client.fetch_unlock_schedule(&url).await.expect("first fetch");
        assert_eq!(first.len(), 1);
        // second call should hit cache; stop the server to ensure cache used
        server.stop_async().await;
        let second = client.fetch_unlock_schedule(&url).await.expect("second fetch");
        assert_eq!(second.len(), 1);
        m.assert_hits(1);
    }
}
