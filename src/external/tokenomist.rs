use serde::Deserialize;
use std::time::Duration;

/// Simple Tokenomist wrapper (placeholder)
#[derive(Debug, Deserialize)]
pub struct UnlockPlan {
    pub token: String,
    pub amount: f64,
    pub timestamp: u64,
}

pub async fn fetch_unlock_schedule(_api_url: &str) -> Result<Vec<UnlockPlan>, reqwest::Error> {
    // Placeholder implementation: implement real HTTP + retry logic here.
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(vec![])
}
