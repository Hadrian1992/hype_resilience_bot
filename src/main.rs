use crate::brain::anomaly::DepthAnomalyDetector;
use crate::brain::mathematics::{compute_bid_depth_usd, compute_imbalance, TradeWindow};
use crate::brain::risk_manager::{Action, CentralRiskManager, RiskConfig, RiskState};
use crate::config::BotConfig;
use crate::storage;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

mod brain;
mod config;
mod execution;
mod external;
mod internal;
mod storage;
mod telemetry;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn json_signal_open(
    id: u64,
    sym: &str,
    kind: &str,
    ts: u64,
    price: f64,
    depth: f64,
    note: &str,
) -> serde_json::Value {
    serde_json::json!({
        "record": "signal_open", "id": id, "symbol": sym, "kind": kind,
        "ts": ts, "price": price, "depth": depth, "note": note,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Offline mode: summarize historical performance of paper signals.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "--replay" {
        return storage::replay_signals();
    }

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
    let (gielda_tx, mut gielda_rx) = mpsc::channel::<serde_json::Value>(2000);
    let (chain_tx, mut chain_rx) = mpsc::channel::<serde_json::Value>(500);
    let (trade_tx, mut trade_rx) = mpsc::channel::<serde_json::Value>(5000);

    // Tokenomist: symbol -> (next_unlock_ts, amount_tokens), odświeżane cyklicznie
    let tokenomist_active = cfg
        .tokenomist_url
        .as_ref()
        .map(|u| !u.trim().is_empty())
        .unwrap_or(false);
    let unlocks_shared = Arc::new(tokio::sync::Mutex::new(HashMap::<String, (u64, f64)>::new()));

    if tokenomist_active {
        let url = cfg.tokenomist_url.clone().unwrap_or_default();
        let poll_secs = cfg.tokenomist_poll_secs.max(60);
        let symbols: Vec<String> = cfg
            .tracked_assets
            .iter()
            .map(|a| a.symbol.to_uppercase())
            .collect();
        let un = unlocks_shared.clone();
        tokio::spawn(async move {
            let client =
                external::tokenomist::TokenomistClient::new(std::time::Duration::from_secs(300));
            loop {
                match client.fetch_unlock_schedule(&url).await {
                    Ok(plans) => {
                        let now = now_secs();
                        let mut next: HashMap<String, (u64, f64)> = HashMap::new();
                        for p in plans {
                            let sym = p.token.to_uppercase();
                            if !symbols.contains(&sym) || p.timestamp < now {
                                continue;
                            }
                            let slot = next.entry(sym).or_insert((u64::MAX, 0.0));
                            if p.timestamp < slot.0 {
                                *slot = (p.timestamp, p.amount);
                            }
                        }
                        {
                            let mut guard = un.lock().await;
                            *guard = next;
                        }
                        info!("tokenomist: unlock schedule refreshed");
                    }
                    Err(e) => warn!("tokenomist: fetch failed: {}", e),
                }
                tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
            }
        });
    } else {
        info!("tokenomist: disabled (no tokenomist_url configured)");
    }

    // RDZEŃ 1: Asynchroniczny pobór danych z giełdy (QuickNode gRPC)
    let grpc_cfg = cfg.clone();
    tokio::spawn(async move {
        internal::grpc_orderbook::start_grpc_orderbook_stream(grpc_cfg, gielda_tx).await;
    });

    // RDZEŃ 2: Cykliczne monitorowanie blockchaina (Alchemy RPC)
    let rpc_cfg = cfg.clone();
    tokio::spawn(async move {
        internal::rpc_vesting::start_rpc_vesting_monitor(rpc_cfg, chain_tx).await;
    });

    // RDZEŃ 2b: strumień wykonanych transakcji (prawdziwy wolumen / VWAP)
    let trades_cfg = cfg.clone();
    tokio::spawn(async move {
        internal::grpc_trades::start_grpc_trades_stream(trades_cfg, trade_tx).await;
    });

    // RDZEŃ 3: Mózg multi-asset (ryzyko + sygnały papierowe + anomalie + metryki)
    tokio::spawn(async move {
        struct AssetBrain {
            rm: CentralRiskManager,
            tw: TradeWindow,
            anomaly: DepthAnomalyDetector,
            price: f64,
            depth: f64,
            imbalance: f64,
            proxy_unlock_usd: f64,
            has_data: bool,
            prev_state_code: u8,
        }

        let mut engine = brain::signals::SignalEngine::new(
            cfg.signal_horizon_secs.max(60),
            cfg.signal_validate_drop_pct.abs().max(1e-6),
            (cfg.signal_horizon_secs / 2).max(60),
        );
        let mut assets: HashMap<String, AssetBrain> = HashMap::new();
        for a in &cfg.tracked_assets {
            let sym = a.symbol.to_uppercase();
            assets.insert(
                sym.clone(),
                AssetBrain {
                    rm: CentralRiskManager::new(RiskConfig {
                        depth_min_usd: 25_000.0,
                        unlock_ratio_warn: 0.10,
                        buyback_budget_ratio: 0.05,
                    }),
                    tw: TradeWindow::new(24),
                    anomaly: DepthAnomalyDetector::new(3.0),
                    price: 0.0,
                    depth: 0.0,
                    imbalance: 0.0,
                    proxy_unlock_usd: 0.0,
                    has_data: false,
                    prev_state_code: 0,
                },
            );
        }

        let name_of = |c: u8| match c {
            1 => "WARN",
            2 => "CRITICAL",
            _ => "OK",
        };

        loop {
            let unlock_snapshot = unlocks_shared.lock().await.clone();
            let mut touched: Option<String> = None;
            tokio::select! {
                Some(book) = gielda_rx.recv() => {
                    telemetry::inc_messages_received();
                    let sym = book["coin"].as_str().unwrap_or("").to_uppercase();

                    let bids: Vec<(f64, f64)> = book["bids"].as_array()
                        .map(|ls| ls.iter().filter_map(|l| Some((l["px"].as_f64()?, l["sz"].as_f64()?))).collect())
                        .unwrap_or_default();
                    let asks: Vec<(f64, f64)> = book["asks"].as_array()
                        .map(|ls| ls.iter().filter_map(|l| Some((l["px"].as_f64()?, l["sz"].as_f64()?))).collect())
                        .unwrap_or_default();
                    if bids.is_empty() { continue; }
                    let best_bid = bids.iter().map(|(px, _)| *px).fold(f64::MIN, f64::max);
                    if !(best_bid.is_finite() && best_bid > 0.0) { continue; }

                    let Some(ab) = assets.get_mut(&sym) else { continue; };
                    ab.price = best_bid;
                    ab.depth = compute_bid_depth_usd(&bids, best_bid, 0.02);
                    ab.imbalance = compute_imbalance(&bids, &asks, best_bid, 0.02);
                    ab.has_data = true;
                    telemetry::set_depth_usd(&sym, ab.depth);
                    telemetry::set_imbalance(&sym, ab.imbalance);

                    let (z, anomalous) = ab.anomaly.update(ab.depth);
                    telemetry::set_anomaly_z(&sym, z);
                    let (p, d) = (ab.price, ab.depth);
                    drop(ab);

                    if anomalous {
                        let now = now_secs();
                        if engine.can_raise(&sym, "ANOMALY", now) {
                            let note = format!("depth z-score spike: depth={:.0} USD", d);
                            let (id, ts, px, dp, nt) =
                                engine.raise(&sym, "ANOMALY", now, p, d, &note);
                            telemetry::inc_paper_signal("ANOMALY", "open");
                            storage::append_jsonl(
                                storage::SIGNALS_FILE,
                                &json_signal_open(id, &sym, "ANOMALY", ts, px, dp, &nt),
                            );
                        }
                    }
                    touched = Some(sym);
                }
                Some(trade) = trade_rx.recv() => {
                    telemetry::inc_messages_received();
                    let sym = trade["coin"].as_str().unwrap_or("").to_uppercase();
                    let px = trade["px"].as_f64().unwrap_or(0.0);
                    let sz = trade["sz"].as_f64().unwrap_or(0.0);
                    if px <= 0.0 { continue; }
                    let Some(ab) = assets.get_mut(&sym) else { continue; };
                    if ab.price <= 0.0 { ab.price = px; }
                    ab.tw.push(now_secs(), px * sz, sz);
                    ab.has_data = true;
                    drop(ab);
                    touched = Some(sym);
                }
                Some(ev) = chain_rx.recv() => {
                    telemetry::inc_messages_received();
                    if ev["type"].as_str() != Some("vesting_transfer") { continue; }
                    let sym = ev["symbol"].as_str().unwrap_or("").to_uppercase();
                    let tokens = ev["amount"].as_f64().unwrap_or(0.0);
                    let Some(ab) = assets.get_mut(&sym) else { continue; };
                    ab.proxy_unlock_usd =
                        ev["window_sum"].as_f64().unwrap_or(0.0) * ab.price;
                    let usd = tokens * ab.price;
                    let (p, d) = (ab.price, ab.depth);
                    drop(ab);

                    if tokens >= cfg.threshold_hype && engine.can_raise(&sym, "WHALE", now_secs()) {
                        let note =
                            format!("vesting transfer {:.0} {} (~{:.0} USD)", tokens, sym, usd);
                        let (id, ts, px, dp, nt) =
                            engine.raise(&sym, "WHALE", now_secs(), p, d, &note);
                        telemetry::inc_paper_signal("WHALE", "open");
                        storage::append_jsonl(
                            storage::SIGNALS_FILE,
                            &json_signal_open(id, &sym, "WHALE", ts, px, dp, &nt),
                        );
                    }
                    touched = Some(sym);
                }
                else => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    continue;
                }
            }

            let Some(sym) = touched else { continue; };

            // ---- ocena ryzyka + eksport metryk dla dotkniętego aktywa ----
            let (volume_24h, depth_v, price_v, upcoming, code, action_taken, prev_code) = {
                let Some(ab) = assets.get_mut(&sym) else { continue; };
                if !ab.has_data { continue; }

                let volume_24h = ab.tw.volume();
                telemetry::set_volume_24h(&sym, volume_24h);
                telemetry::set_vwap_24h(&sym, ab.tw.vwap());
                telemetry::set_trades_count_24h(&sym, ab.tw.count());

                let upcoming = if tokenomist_active {
                    match unlock_snapshot.get(&sym) {
                        Some((ts, amt)) if *ts <= now_secs() + 7 * 86_400 => *amt * ab.price,
                        _ => 0.0,
                    }
                } else {
                    ab.proxy_unlock_usd
                };
                telemetry::set_pending_unlock_usd(&sym, upcoming);

                let (state, action) = ab.rm.evaluate(volume_24h, ab.depth, upcoming);
                let code = match state {
                    RiskState::Ok => 0u8,
                    RiskState::Warn => 1u8,
                    RiskState::Critical => 2u8,
                };
                telemetry::set_risk_state(&sym, code);
                let prev = ab.prev_state_code;
                ab.prev_state_code = code;
                (volume_24h, ab.depth, ab.price, upcoming, code, action, prev)
            };

            // ---- alerty Telegram przy zmianie stanu ----
            if code != prev_code {
                info!("risk [{}]: {} -> {}", sym, name_of(prev_code), name_of(code));
                match &action_taken {
                    Some(Action::AlertTelegram(text)) => {
                        warn!("RISK ALERT [{}]: {}", sym, text);
                        if let Some(tg) = &cfg.telegram {
                            if let (Some(token), Some(chat)) = (&tg.bot_token, &tg.chat_id) {
                                let full = format!("[HYPE-RESILIENCE][{}] {}", sym, text);
                                if let Err(e) =
                                    execution::telegram::send_telegram_alert(token, chat, &full)
                                        .await
                                {
                                    error!("telegram send failed: {}", e);
                                }
                            }
                        }
                    }
                    Some(Action::ThrottleBuyback { recommended_usd }) => {
                        warn!(
                            "risk WARN [{}]: recommended buyback throttle ${:.2}",
                            sym, recommended_usd
                        );
                    }
                    None => {}
                }
            }

            // ---- papierowy sygnał ryzyka przy wejściu w WARN/CRITICAL ----
            if code > prev_code {
                let kind = if code == 2 { "CRITICAL" } else { "WARN" };
                let now = now_secs();
                if engine.can_raise(&sym, kind, now) {
                    let note = format!(
                        "state={} depth={:.0} vol24={:.0} unlock={:.0}",
                        name_of(code), depth_v, volume_24h, upcoming
                    );
                    let (id, ts, px, dp, nt) =
                        engine.raise(&sym, kind, now, price_v, depth_v, &note);
                    telemetry::inc_paper_signal(kind, "open");
                    storage::append_jsonl(
                        storage::SIGNALS_FILE,
                        &json_signal_open(id, &sym, kind, ts, px, dp, &nt),
                    );
                }
            }

            // ---- ocena dojrzałych sygnałów papierowych ----
            let now = now_secs();
            let prices: Vec<(String, f64)> = assets
                .iter()
                .filter(|(_, a)| a.price > 0.0)
                .map(|(s, a)| (s.clone(), a.price))
                .collect();
            for cs in engine.on_tick(now, &prices) {
                let res = if cs.validated { "validated" } else { "invalidated" };
                telemetry::inc_paper_signal(&cs.kind, res);
                if let Ok(mut v) = serde_json::to_value(&cs) {
                    v["record"] = serde_json::Value::String("signal_closed".into());
                    storage::append_jsonl(storage::SIGNALS_FILE, &v);
                }
                info!(
                    "paper signal {} [{}] {} {}: change {:.2}% -> {}",
                    cs.id,
                    cs.kind,
                    cs.symbol,
                    res,
                    cs.change_pct * 100.0,
                    if cs.validated { "HIT" } else { "MISS" }
                );
            }
            telemetry::set_open_signals(engine.open_count());
        }
    });

    // Trzymamy wątek główny przy życiu
    tokio::signal::ctrl_c().await?;
    println!("🛑 System bezpiecznie wyłączony.");
    Ok(())
}
