use crate::config::BotConfig;
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use std::time::Duration;

/// gRPC orderbook stream skeleton.
pub async fn start_grpc_orderbook_stream(cfg: BotConfig, tx: Sender<Value>) {
    // Placeholder implementation: wire up tonic gRPC client, StreamL2Book subscription,
    // decompress zstd frames, parse JSON and send to channel.
    loop {
        let dummy = serde_json::json!({"stream":"dummy_l2book_update"});
        if let Err(e) = tx.send(dummy).await {
            eprintln!("grpc_orderbook: failed to send on channel: {}", e);
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
