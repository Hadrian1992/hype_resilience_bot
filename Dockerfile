# Multi-stage Dockerfile for hype_resilience_bot

FROM rust:1-slim AS builder
WORKDIR /usr/src/hype_resilience_bot

# Install dependencies required for building (incl. protobuf compiler for tonic-build)
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config libssl-dev ca-certificates protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy source
COPY . .

# Build in release
RUN cargo build --release

# Runtime image
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/hype_resilience_bot

# Copy binary
COPY --from=builder /usr/src/hype_resilience_bot/target/release/hype_resilience_bot ./hype_resilience_bot

# Create state dir
RUN mkdir -p /opt/hype_resilience_bot/state

# Entrypoint
ENV RUST_LOG=info
EXPOSE 9898
CMD ["/opt/hype_resilience_bot/hype_resilience_bot"]
