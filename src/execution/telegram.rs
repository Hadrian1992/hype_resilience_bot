use std::time::Duration;

use reqwest::Client;

/// Sends a Telegram alert message with retry, exponential backoff and jitter.
///
/// Retries up to 5 attempts on network errors and HTTP 5xx/429 responses.
/// Other client errors (e.g. 4xx) are returned immediately without retrying.
pub async fn send_telegram_alert(
    bot_token: &str,
    chat_id: &str,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::new();
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    let body = serde_json::json!({"chat_id": chat_id, "text": text});

    let max_attempts = 5u32;
    let mut attempt = 0u32;
    let mut backoff = 1u64;

    loop {
        attempt += 1;

        let res = client
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await;

        match res {
            Ok(r) => {
                if r.status().is_success() {
                    return Ok(());
                }
                // Client errors other than rate limiting won't succeed on retry.
                if !r.status().is_server_error() && r.status().as_u16() != 429 {
                    return Err(format!("telegram client error: {}", r.status()).into());
                }
            }
            Err(e) => {
                if attempt >= max_attempts {
                    return Err(e.into());
                }
            }
        }

        if attempt >= max_attempts {
            return Err(format!("telegram request failed after {} attempts", attempt).into());
        }

        // backoff with jitter
        let jitter = rand::random::<u64>() % 500;
        let wait_ms = (backoff * 1000) + jitter;
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        backoff = (backoff * 2).min(30);
    }
}

