use crate::config::BotConfig;
use serde_json::Value;
use tokio::sync::mpsc::Sender;
use std::time::Duration;

/// RPC vesting monitor skeleton.
pub async fn start_rpc_vesting_monitor(cfg: BotConfig, tx: Sender<Value>) {
    // This is a placeholder loop. Implement alloy client + eth_getLogs here.
    loop {
        // TODO: call alloy provider, eth_getLogs filtering tracked vesting contracts
        // If detected events -> tx.send(json!({...})).await
        let dummy = serde_json::json!({"event":"dummy_vesting_check"});
        if let Err(e) = tx.send(dummy).await {
            eprintln!("rpc_vesting: failed to send on channel: {}", e);
            return;
        }
        tokio::time::sleep(Duration::from_secs(cfg.poll_interval_seconds)).await;
    }
}
