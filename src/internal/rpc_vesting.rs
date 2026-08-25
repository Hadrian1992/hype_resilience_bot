use crate::config::BotConfig;
use ethers::prelude::*;
use ethers::types::{Filter, Log, H256, Address, U256, BlockNumber};
use serde_json::json;
use serde_json::Value;
use std::collections::VecDeque;
use std::fs::{create_dir_all, File};
use std::io::{Read, Write};
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tracing::{error, info, warn};

const STATE_DIR: &str = "state";
const LAST_BLOCK_FILE: &str = "state/last_block.json";
const DECIMALS: u32 = 18;

#[derive(serde::Deserialize, serde::Serialize)]
struct LastBlockState {
    last_block: u64,
}

fn load_last_block() -> Option<u64> {
    if !Path::new(LAST_BLOCK_FILE).exists() {
        return None;
    }
    match File::open(LAST_BLOCK_FILE) {
        Ok(mut f) => {
            let mut s = String::new();
            if f.read_to_string(&mut s).is_ok() {
                if let Ok(st) = serde_json::from_str::<LastBlockState>(&s) {
                    return Some(st.last_block);
                }
            }
            None
        }
        Err(_) => None,
    }
}

fn save_last_block(block: u64) -> Result<(), Box<dyn std::error::Error>> {
    create_dir_all(STATE_DIR)?;
    let tmp = format!("{}/last_block.json.tmp", STATE_DIR);
    let final_path = LAST_BLOCK_FILE;
    let mut f = File::create(&tmp)?;
    let s = serde_json::to_string(&LastBlockState { last_block: block })?;
    f.write_all(s.as_bytes())?;
    f.flush()?;
    std::fs::rename(tmp, final_path)?;
    Ok(())
}

/// Start the RPC vesting monitor. This function will run forever until the task is cancelled.
/// It polls eth_getLogs for Transfer events from vesting contracts and emits parsed events to tx.
pub async fn start_rpc_vesting_monitor(cfg: BotConfig, tx: Sender<Value>) {
    // Build provider using ethers
    let provider = match Provider::<Http>::try_from(cfg.rpc_url.clone()) {
        Ok(p) => p.interval(Duration::from_millis(0u64)),
        Err(e) => {
            error!("rpc_vesting: failed to create provider: {}", e);
            return;
        }
    };

    // Parse HYPE token address
    let hype_addr = match Address::from_str(&cfg.hype_token_address) {
        Ok(a) => a,
        Err(e) => {
            error!("rpc_vesting: invalid HYPE token address: {}", e);
            return;
        }
    };

    // Parse vesting addresses
    let mut vesting_addresses: Vec<Address> = Vec::new();
    for a in cfg.tracked_assets.iter() {
        if let Ok(addr) = Address::from_str(&a.vesting_contract) {
            vesting_addresses.push(addr);
        } else {
            warn!("rpc_vesting: skipping invalid vesting address for {}: {}", a.symbol, a.vesting_contract);
        }
    }

    // Topic0 = Transfer(address,address,uint256)
    let topic0: H256 = H256::from_slice(ethers::utils::keccak256(b"Transfer(address,address,uint256)").as_slice());

    // Initialize last_processed_block
    let mut last_processed_block: u64 = if let Some(b) = load_last_block() { b } else {
        match provider.get_block_number().await {
            Ok(n) => {
                let n_u64 = n.as_u64();
                if n_u64 > 10 { n_u64 - 10 } else { 0 }
            }
            Err(e) => {
                error!("rpc_vesting: failed to get latest block number: {}", e);
                0
            }
        }
    };

    info!("rpc_vesting: starting from block {}", last_processed_block);

    // Sliding window of unlocks: VecDeque of (timestamp_seconds, amount_tokens)
    let mut window: VecDeque<(u64, f64)> = VecDeque::new();
    let mut window_sum: f64 = 0.0;
    let window_seconds = cfg.window_hours * 3600;

    // Exponential backoff parameters
    let mut backoff_base_secs: u64 = 1;

    loop {
        let res = async {
            // determine latest block
            let latest_bn = provider.get_block_number().await?;
            let latest = latest_bn.as_u64();
            if latest <= last_processed_block {
                // nothing new
                return Ok::<(), ProviderError>(());
            }

            let fromb = last_processed_block + 1;
            let tob = latest;

            // Build filter: address = hype token, topic0 = Transfer, topic1 = vesting addresses
            let mut filter = Filter::new()
                .address(hype_addr)
                .topic0(ValueOrArray::Value(topic0));

            // For topic1 (from) we build array of H256 topics
            if !vesting_addresses.is_empty() {
                let topics1: Vec<H256> = vesting_addresses.iter().map(|addr| {
                    // indexed address is padded to 32 bytes
                    let mut bytes = [0u8; 32];
                    bytes[12..].copy_from_slice(addr.as_bytes());
                    H256::from(bytes)
                }).collect();
                filter = filter.topic1(ValueOrArray::Array(topics1.into()));
            }

            // set block range
            filter = filter.from_block(BlockNumber::Number(fromb.into())).to_block(BlockNumber::Number(tob.into()));

            // fetch logs
            let logs: Vec<Log> = provider.get_logs(&filter).await?;

            for log in logs.into_iter() {
                // parse amount from data (32 bytes)
                let raw_data: &[u8] = log.data.as_ref();
                let amount_wei: U256 = if raw_data.len() >= 32 {
                    U256::from_big_endian(&raw_data[raw_data.len() - 32..])
                } else {
                    U256::zero()
                };
                let amount_f64 = if amount_wei.is_zero() {
                    0.0
                } else {
                    // convert U256 -> f64 with decimals
                    let denom = 10u128.pow(DECIMALS) as f64;
                    // careful conversion: U256 -> string -> f64
                    let s = amount_wei.to_string();
                    match s.parse::<f64>() {
                        Ok(v) => v / denom,
                        Err(_) => {
                            // fallback large numbers
                            0.0
                        }
                    }
                };

                // get block timestamp
                let ts_secs = if let Some(bn) = log.block_number {
                    if let Ok(Some(block)) = provider.get_block(bn).await {
                        block.timestamp.as_u64()
                    } else {
                        // fallback to now
                        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
                    }
                } else {
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
                };

                // push into sliding window
                window.push_back((ts_secs, amount_f64));
                window_sum += amount_f64;

                // evict old
                while let Some((t, amt)) = window.front() {
                    if *t + window_seconds < ts_secs {
                        let (_t, a) = window.pop_front().unwrap();
                        window_sum -= a;
                    } else {
                        break;
                    }
                }

                // build event json
                let parsed = json!({
                    "type": "vesting_transfer",
                    "from": log.topics.get(1).map(|t| format!("0x{}", hex::encode(&t.as_bytes()[12..]))),
                    "to": log.topics.get(2).map(|t| format!("0x{}", hex::encode(&t.as_bytes()[12..]))),
                    "amount": amount_f64,
                    "block_number": log.block_number.map(|n| n.as_u64()),
                    "tx_hash": log.transaction_hash.map(|h| format!("0x{}", hex::encode(h.as_bytes()))),
                    "window_sum": window_sum,
                    "window_hours": cfg.window_hours,
                });

                // send
                if let Err(e) = tx.send(parsed).await {
                    warn!("rpc_vesting: failed to send event to channel: {}", e);
                }

                // check threshold
                if window_sum > cfg.threshold_hype {
                    info!("rpc_vesting: threshold exceeded: {} > {}", window_sum, cfg.threshold_hype);
                    let alert = json!({"type":"claim_rate_spike","window_sum":window_sum});
                    if let Err(e) = tx.send(alert).await {
                        warn!("rpc_vesting: failed to send alert: {}", e);
                    }
                }
            }

            // update last_processed_block
            last_processed_block = tob;
            // persist
            if let Err(e) = save_last_block(last_processed_block) {
                warn!("rpc_vesting: failed to save last block: {}", e);
            }

            Ok(()) as Result<(), ProviderError>
        }
        .await;

        match res {
            Ok(_) => {
                // reset backoff
                backoff_base_secs = 1;
                // sleep until next poll
                sleep(Duration::from_secs(cfg.poll_interval_seconds)).await;
            }
            Err(e) => {
                error!("rpc_vesting: provider error: {}", e);
                // backoff with jitter (rand::random - ThreadRng is not Send and
                // must not be held across an await inside a spawned task)
                let jitter: u64 = rand::random::<u64>() % 3;
                let wait = (backoff_base_secs + jitter).min(120);
                warn!("rpc_vesting: backing off for {}s", wait);
                sleep(Duration::from_secs(wait)).await;
                backoff_base_secs = (backoff_base_secs * 2).min(120);
            }
        }
    }
}
