//! Streams executed trades over gRPC into normalized JSON for the brain.

use crate::config::BotConfig;
use crate::internal::grpc_orderbook::{order_book_streaming_client::OrderBookStreamingClient, AllTradesRequest};
use crate::telemetry;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tonic::transport::Channel;
use tonic::Request;
use tracing::{error, info};

pub async fn start_grpc_trades_stream(cfg: BotConfig, tx: Sender<Value>) {
    let mut backoff = 1u64;
    loop {
        match run_stream(&cfg, tx.clone()).await {
            Ok(_) => {
                info!("grpc_trades: stream ended gracefully");
                backoff = 1;
            }
            Err(e) => {
                error!("grpc_trades: stream error: {}", e);
                telemetry::inc_reconnects();
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        }
    }
}

async fn run_stream(
    cfg: &BotConfig,
    tx: Sender<Value>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("grpc_trades: connecting to {}", cfg.grpc_url);
    let channel = Channel::from_shared(cfg.grpc_url.clone())?.connect().await?;
    let mut client = OrderBookStreamingClient::new(channel);

    let req = AllTradesRequest { coin: String::new() };
    let mut stream = client.stream_trades(Request::new(req)).await?.into_inner();

    loop {
        let msg = tokio::time::timeout(Duration::from_secs(30), stream.message()).await;
        match msg {
            Err(_) => return Err("trades stream idle > 30s".into()),
            Ok(Err(status)) => return Err(status.into()),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(t))) => {
                let trade = json!({
                    "type": "trade",
                    "coin": t.coin.to_uppercase(),
                    "px": t.px.parse::<f64>().unwrap_or(0.0),
                    "sz": t.sz.parse::<f64>().unwrap_or(0.0),
                    "side": t.side,
                    "tid": t.tid,
                    "time": t.time,
                });
                if tx.send(trade).await.is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}
