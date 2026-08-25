use std::convert::Infallible;
use std::net::SocketAddr;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use once_cell::sync::Lazy;
use prometheus::{Encoder, Gauge, IntCounter, Registry, TextEncoder};
use tracing_subscriber::EnvFilter;

static MET_REGISTRY: Lazy<Registry> = Lazy::new(|| Registry::new());
static MESSAGES_RECEIVED: Lazy<IntCounter> = Lazy::new(|| {
    let c = IntCounter::new("messages_received", "Number of messages received").unwrap();
    MET_REGISTRY.register(Box::new(c.clone())).ok();
    c
});
static MESSAGES_DROPPED: Lazy<IntCounter> = Lazy::new(|| {
    let c = IntCounter::new("messages_dropped", "Number of messages dropped").unwrap();
    MET_REGISTRY.register(Box::new(c.clone())).ok();
    c
});
static RECONNECTS: Lazy<IntCounter> = Lazy::new(|| {
    let c = IntCounter::new("reconnects", "Number of reconnections").unwrap();
    MET_REGISTRY.register(Box::new(c.clone())).ok();
    c
});
static RPC_ERRORS: Lazy<IntCounter> = Lazy::new(|| {
    let c = IntCounter::new("rpc_errors", "Number of RPC errors").unwrap();
    MET_REGISTRY.register(Box::new(c.clone())).ok();
    c
});
static DEPTH_USD: Lazy<Gauge> = Lazy::new(|| {
    let g = Gauge::new("depth_usd", "Orderbook depth in USD").unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static VOLUME_24H: Lazy<Gauge> = Lazy::new(|| {
    let g = Gauge::new("volume_24h", "24h rolling volume in USD").unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static RISK_STATE: Lazy<Gauge> = Lazy::new(|| {
    let g = Gauge::new("risk_state", "Current risk state (0=OK,1=WARN,2=CRITICAL)").unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});

pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().json().with_env_filter(filter).init();
}

pub async fn run_metrics_server(
    addr: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr = addr.parse()?;
    let registry = MET_REGISTRY.clone();

    let make_svc = make_service_fn(move |_conn| {
        let reg = registry.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |_req: Request<Body>| {
                let reg = reg.clone();
                async move {
                    let encoder = TextEncoder::new();
                    let metric_families = reg.gather();
                    let mut buffer = Vec::new();
                    encoder
                        .encode(&metric_families, &mut buffer)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                    let response = Response::builder()
                        .status(200)
                        .header("Content-Type", encoder.format_type())
                        .body(Body::from(buffer))
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(response)
                }
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);
    tracing::info!("Metrics server listening on {}", addr);
    server.await?;
    Ok(())
}

// helper setters
pub fn inc_messages_received() {
    MESSAGES_RECEIVED.inc();
}
pub fn inc_messages_dropped() {
    MESSAGES_DROPPED.inc();
}
pub fn inc_reconnects() {
    RECONNECTS.inc();
}
pub fn inc_rpc_errors() {
    RPC_ERRORS.inc();
}
pub fn set_depth_usd(v: f64) {
    DEPTH_USD.set(v);
}
pub fn set_volume_24h(v: f64) {
    VOLUME_24H.set(v);
}
pub fn set_risk_state(s: u8) {
    RISK_STATE.set(s as f64);
}
