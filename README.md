# hype_resilience_bot

Event-driven Hyperliquid monitoring bot (skeleton).

This repository was generated with a project skeleton for the hype_resilience_bot described in the design specification. It contains placeholders and skeleton modules for:

- gRPC orderbook stream (src/internal/grpc_orderbook.rs)
- RPC vesting monitor (src/internal/rpc_vesting.rs)
- Core brain modules (src/brain)
- External Tokenomist wrapper (src/external/tokenomist.rs)
- Telegram execution module (src/execution/telegram.rs)

Next steps:
1. Add the real proto file to proto/orderbook.proto from quiknode-labs/hypercore-grpc-examples.
2. Implement the tonic gRPC client in src/internal/grpc_orderbook.rs using generated types.
3. Implement the alloy-based eth_getLogs client in src/internal/rpc_vesting.rs.
4. Fill mathematics and risk_manager with production logic.

## Quick start (Docker Compose: bot + Prometheus + Grafana)

```powershell
Copy-Item .env.example .env   # then fill in your keys (never commit .env!)
docker compose up --build
```

After startup:

| Service   | URL                            | Notes                                   |
|-----------|--------------------------------|-----------------------------------------|
| Bot       | http://localhost:9898/metrics  | raw Prometheus metrics                  |
| Prometheus| http://localhost:9090          | scrapes the bot every 5 s               |
| Grafana   | http://localhost:3000          | user `admin`, password from `GRAFANA_PASSWORD` |

The dashboard **"HYPE Resilience Bot"** (folder `HYPE`) is provisioned automatically:
bid depth, risk-state gauge, 24h volume estimate, message flow and stability panels.

## What's inside

- **Faza 1** – real trades stream (24h volume + VWAP + trade count), order-book
  imbalance (±2%), Tokenomist unlock schedule wired into the brain,
  multi-asset support (per-asset risk manager, metrics labelled by `asset`)
- **Faza 2** – paper signals (`WARN`, `CRITICAL`, `WHALE`, `ANOMALY`) graded
  automatically against realized price action over a configurable horizon
- **Faza 3** – signals persisted to `state/signals.jsonl` + offline replay summary
- **Faza 4** – EWMA anomaly detection on depth (`anomaly_depth_z`) and automatic
  WebSocket fallback (`wss://api.hyperliquid.xyz/ws`) after repeated gRPC failures

## Paper-signal replay

```powershell
cargo run --release -- --replay          # or: docker compose run --rm bot --replay
```

Prints overall and per-kind hit rates of closed paper signals.

Extra environment variables: `TOKENOMIST_URL` (optional, enables the
`pending_unlock_usd` metric) and `WS_FALLBACK_URL`.


