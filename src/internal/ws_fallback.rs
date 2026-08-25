//! WebSocket fallback for the L2 book: subscribes the public Hyperliquid WS
//! when the gRPC stream keeps failing. Emits messages in the SAME normalized
//! JSON shape as grpc_orderbook so the brain stays unaware of the transport.

use crate::config::BotConfig;
use futures_util::SinkExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio_tungstenite::tungstenite::Message;
use tracing::info;

const IDLE_TIMEOUT_SECS: u64 = 30;

/// Runs a single WS session until error / idle timeout / channel close.
/// The gRPC supervisor decides when to re-enter this fallback.
pub async fn run_until_error(
    cfg: &BotConfig,
    tx: Sender<Value>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = cfg.ws_fallback_url.clone().unwrap_or_default();
    if url.trim().is_empty() {
        return Err("websocket fallback disabled (ws_fallback_url not set)".into());
    }

    let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str()).await?;

    for asset in &cfg.tracked_assets {
        let sub = json!({
            "method": "subscribe",
            "subscription": { "type": "l2Book", "coin": asset.symbol }
        });
        ws.send(Message::Text(sub.to_string())).await?;
    }
    info!(
        "ws_fallback: subscribed {} assets on {}",
        cfg.tracked_assets.len(),
        url
    );

    loop {
        let next = tokio::time::timeout(Duration::from_secs(IDLE_TIMEOUT_SECS), ws.next()).await;
        match next {
            Err(_) => return Err(format!("ws idle > {}s", IDLE_TIMEOUT_SECS).into()),
            Ok(None) => return Err("ws stream ended".into()),
            Ok(Some(Err(e))) => return Err(e.into()),
            Ok(Some(Ok(Message::Text(txt)))) => {
                match serde_json::from_str::<Value>(&txt) {
                    Ok(v) if v["channel"].as_str() == Some("l2Book") => {
                        if let Some(norm) = normalize_l2book(&v) {
                            if tx.send(norm).await.is_err() {
                                return Err("brain channel closed".into());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// Normalizes Hyperliquid WS l2Book payload into the internal book shape.
fn normalize_l2book(v: &Value) -> Option<Value> {
    let data = v.get("data")?;
    let coin = data["coin"].as_str()?.to_uppercase();
    let levels = data["levels"].as_array()?;
    if levels.len() < 2 {
        return None;
    }
    let bids = level_list(&levels[0]);
    let asks = level_list(&levels[1]);
    Some(json!({
        "coin": coin,
        "time": data["time"].as_i64().unwrap_or(0),
        "block_number": Value::Null,
        "bids": bids,
        "asks": asks,
    }))
}

fn level_list(arr: &Value) -> Vec<Value> {
    arr.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    let px = l["px"].as_str()?.parse::<f64>().ok()?;
                    let sz = l["sz"].as_str()?.parse::<f64>().ok()?;
                    Some(json!({ "px": px, "sz": sz, "n": l["n"].as_u64().unwrap_or(0) }))
                })
                .collect()
        })
        .unwrap_or_default()
}

