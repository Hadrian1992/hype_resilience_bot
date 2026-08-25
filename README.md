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
