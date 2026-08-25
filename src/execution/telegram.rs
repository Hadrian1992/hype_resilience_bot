use reqwest::Client;
use std::time::Duration;

pub async fn send_telegram_alert(bot_token: &str, chat_id: &str, text: &str) -> Result<(), reqwest::Error> {
    let client = Client::new();
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    let body = serde_json::json!({"chat_id": chat_id, "text": text});
    let _ = client.post(&url)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}
