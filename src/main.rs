use crate::config::BotConfig;
use tokio::sync::mpsc;

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

    // RDZEŃ 3: Główny Mózg Systemu (Konsument danych - CentralRiskManager)
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(gielda_data) = gielda_rx.recv() => {
                    // TODO: Aktualizacja lokalnego modelu płynności
                    println!("[Giełda] got data: {}", gielda_data);
                }
                Some(onchain_alert) = blockchain_rx.recv() => {
                    // TODO: Wyzwolenie natychmiastowej kalkulacji ryzyka absorpcji
                    println!("[Blockchain] got alert: {}", onchain_alert);
                }
                else => {
                    // No channels ready - sleep briefly
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
        }
    });

    // Trzymamy wątek główny przy życiu
    tokio::signal::ctrl_c().await?;
    println!("🛑 System bezpiecznie wyłączony.");
    Ok(())
}
