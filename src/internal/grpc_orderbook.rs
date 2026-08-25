use crate::config::BotConfig;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc::Sender, Mutex};
use tonic::transport::Channel;
use tonic::Request;

use tracing::{error, info, warn};

// Include generated protobuf definitions (package = "hyperliquid")
tonic::include_proto!("hyperliquid");

const RING_CAPACITY: usize = 5_000; // internal ring buffer capacity
const HEARTBEAT_TIMEOUT_SECS: u64 = 5;

pub async fn start_grpc_orderbook_stream(cfg: BotConfig, tx: Sender<Value>) {
    let mut backoff_secs = 1u64;
    loop {
        match run_stream(&cfg, tx.clone()).await {
            Ok(_) => {
                info!("grpc_orderbook: stream ended gracefully");
                backoff_secs = 1;
            }
            Err(e) => {
                error!("grpc_orderbook: stream error: {}", e);
                // exponential backoff with cap
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(60);
            }
        }
    }
}

async fn run_stream(cfg: &BotConfig, tx: Sender<Value>) -> Result<(), Box<dyn std::error::Error>> {
    let uri = cfg.grpc_url.clone();
    info!("grpc_orderbook: connecting to {}", uri);

    let channel = Channel::from_shared(uri.clone())?.connect().await?;
    let mut client = order_book_streaming_client::OrderBookStreamingClient::new(channel);

    // Prepare request: subscribe all coins by providing empty coin string
    let req = L2BookRequest {
        coin: "".to_string(),
        n_levels: 20u32,
        n_sig_figs: None,
        mantissa: None,
    };

    let mut stream = client.stream_l2_book(Request::new(req)).await?.into_inner();

    let ring = Arc::new(Mutex::new(VecDeque::<Value>::with_capacity(RING_CAPACITY)));
    let last_msg = Arc::new(Mutex::new(Instant::now()));

    // spawn flusher task that tries to forward messages from ring to tx
    {
        let ring_cloned = ring.clone();
        let tx_cloned = tx.clone();
        tokio::spawn(async move {
            loop {
                // try to pop and send while channel not full
                let mut guard = ring_cloned.lock().await;
                if let Some(item) = guard.pop_front() {
                    match tx_cloned.try_send(item) {
                        Ok(_) => { /* sent */ }
                        Err(e) => {
                            // channel full or closed: push back the item and sleep briefly
                            // push to front to preserve order
                            guard.push_front(e.into_inner());
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                } else {
                    drop(guard);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        });
    }

    // process incoming stream with an in-loop heartbeat watchdog.
    // On heartbeat timeout we return an error so the outer loop can reconnect
    // (dropping the stream/client closes the underlying connection).
    loop {
        tokio::select! {
            msg = stream.message() => {
                let msg = match msg {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(e) => return Err(e.into()),
                };

                // update heartbeat
                {
                    let mut lm = last_msg.lock().await;
                    *lm = Instant::now();
                }

                // Build JSON payload
                let mut bids = Vec::new();
                for lvl in msg.bids.iter() {
                    // parse price and size as strings in proto
                    let px = lvl.px.parse::<f64>().unwrap_or(0.0);
                    let sz = lvl.sz.parse::<f64>().unwrap_or(0.0);
                    bids.push(serde_json::json!({"px": px, "sz": sz, "n": lvl.n}));
                }
                let mut asks = Vec::new();
                for lvl in msg.asks.iter() {
                    let px = lvl.px.parse::<f64>().unwrap_or(0.0);
                    let sz = lvl.sz.parse::<f64>().unwrap_or(0.0);
                    asks.push(serde_json::json!({"px": px, "sz": sz, "n": lvl.n}));
                }

                let payload = serde_json::json!({
                    "coin": msg.coin,
                    "time": msg.time,
                    "block_number": msg.block_number,
                    "bids": bids,
                    "asks": asks,
                });

                // push into ring buffer (drop oldest if capacity exceeded)
                {
                    let mut guard = ring.lock().await;
                    if guard.len() >= RING_CAPACITY {
                        // drop oldest
                        guard.pop_front();
                    }
                    guard.push_back(payload);
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(HEARTBEAT_TIMEOUT_SECS)) => {
                let elapsed = { last_msg.lock().await.elapsed() };
                if elapsed.as_secs() >= HEARTBEAT_TIMEOUT_SECS {
                    warn!("grpc_orderbook: heartbeat timeout ({}s) - forcing reconnect", elapsed.as_secs());
                    return Err("heartbeat timeout".into());
                }
            }
        }
    }

    Ok(())
}
