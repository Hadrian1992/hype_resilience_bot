use crate::brain::mathematics::{compute_bid_depth_usd, VolumeWindow};
use crate::brain::risk_manager::{Action, CentralRiskManager, RiskConfig, RiskState};
use crate::config::BotConfig;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

mod brain;
mod config;
mod execution;
mod external;
mod internal;
mod telemetry;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦀 System uruchomiony. Alokacja wątków na rdzeniach procesora...");

    let cfg = BotConfig::load_from_file(Some("config.json"))?;

    // Telemetria: logowanie strukturalne + serwer metryk Prometheus (/metrics)
    telemetry::init_tracing();
    let metrics_addr =
        std::env::var("METRICS_ADDR").unwrap_or_else(|_| "0.0.0.0:9898".to_string());
    tokio::spawn(async move {
        if let Err(e) = telemetry::run_metrics_server(&metrics_addr).await {
            eprintln!("metrics server stopped: {}", e);
        }
    });

    // Kanały komunikacji
    let (gielda_tx, mut gielda_rx) = mpsc::channel::<serde_json::Value>(1000);
    let (blockchain_tx, mut blockchain_rx) = mpsc::channel::<serde_json::Value>(100);

    // RDZEŃ 1: Asynchroniczny pobór danych z giełdy (QuickNode gRPC)
    let grpc_cfg = cfg.clone();
    tokio::spawn(async move {
        internal::grpc_orderbook::start_grpc_orderbook_stream(grpc_cfg, gielda_tx).await;
    });

    // RDZEŃ 2: Cykliczne monitorowanie blockchaina (Alchemy RPC)
    let rpc_cfg = cfg.clone();
    tokio::spawn(async move {
        internal::rpc_vesting::start_rpc_vesting_monitor(rpc_cfg, blockchain_tx).await;
    });

    // RDZEŃ 3: Główny Mózg Systemu (CentralRiskManager + metryki + alerty)
    tokio::spawn(async move {
        let risk_cfg = RiskConfig {
            depth_min_usd: 25_000.0,
            unlock_ratio_warn: 0.10,
            buyback_budget_ratio: 0.05,
        };
        let mut rm = CentralRiskManager::new(risk_cfg);
        let mut volume = VolumeWindow::new(24);
        let mut last_price = 0.0f64;
        let mut last_depth = 0.0f64;
        let mut pending_unlock_usd = 0.0f64;
        let mut has_market_data = false;
        let mut prev_state_code = 0u8;

        loop {
            tokio::select! {
                Some(book) = gielda_rx.recv() => {
                    telemetry::inc_messages_received();

                    // L2 book -> depth w USD (2% pod najlepszym bidem)
                    let bids: Vec<(f64, f64)> = book["bids"]
                        .as_array()
                        .map(|levels| {
                            levels
                                .iter()
                                .filter_map(|l| Some((l["px"].as_f64()?, l["sz"].as_f64()?)))
                                .collect()
                        })
                        .unwrap_or_default();

                    if !bids.is_empty() {
                        let best_bid = bids.iter().map(|(px, _)| *px).fold(f64::MIN, f64::max);
                        if best_bid > 0.0 && best_bid.is_finite() {
                            last_price = best_bid;
                            last_depth = compute_bid_depth_usd(&bids, best_bid, 0.02);
                            telemetry::set_depth_usd(last_depth);
                            has_market_data = true;
                        }
                    }
                }
                Some(event) = blockchain_rx.recv() => {
                    telemetry::inc_messages_received();

                    if event["type"].as_str() == Some("vesting_transfer") {
                        let tokens = event["amount"].as_f64().unwrap_or(0.0);
                        let usd = tokens * last_price;
                        let now_secs = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        volume.push_trade(now_secs, usd);
                        pending_unlock_usd =
                            event["window_sum"].as_f64().unwrap_or(0.0) * last_price;
                    }
                }
                else => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    continue;
                }
            }

            if !has_market_data {
                continue;
            }

            // Ocena ryzyka + aktualizacja metryk
            let volume_24h = volume.total_24h(None);
            telemetry::set_volume_24h(volume_24h);

            let (state, action) = rm.evaluate(volume_24h, last_depth, pending_unlock_usd);
            let state_code = match state {
                RiskState::Ok => 0u8,
                RiskState::Warn => 1u8,
                RiskState::Critical => 2u8,
            };
            telemetry::set_risk_state(state_code);

            // Alerty tylko przy ZMIANIE stanu (bez spamu)
            if state_code != prev_state_code {
                info!(
                    "risk state changed: {} -> {}",
                    match prev_state_code {
                        1 => "WARN",
                        2 => "CRITICAL",
                        _ => "OK",
                    },
                    match state_code {
                        1 => "WARN",
                        2 => "CRITICAL",
                        _ => "OK",
                    }
                );
                match action {
                    Some(Action::AlertTelegram(text)) => {
                        warn!("RISK ALERT: {}", text);
                        if let Some(tg) = &cfg.telegram {
                            if let (Some(token), Some(chat)) = (&tg.bot_token, &tg.chat_id) {
                                let full = format!("[HYPE-RESILIENCE] {}", text);
                                if let Err(e) =
                                    execution::telegram::send_telegram_alert(token, chat, &full)
                                        .await
                                {
                                    error!("failed to send telegram alert: {}", e);
                                }
                            }
                        }
                    }
                    Some(Action::ThrottleBuyback { recommended_usd }) => {
                        warn!(
                            "risk WARN: recommended buyback throttle: ${:.2}",
                            recommended_usd
                        );
                    }
                    None => {}
                }
                prev_state_code = state_code;
            }
        }
    });

    // Trzymamy wątek główny przy życiu
    tokio::signal::ctrl_c().await?;
    println!("🛑 System bezpiecznie wyłączony.");
    Ok(())
}
