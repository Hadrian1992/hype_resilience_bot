use std::convert::Infallible;
use std::net::SocketAddr;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use once_cell::sync::Lazy;
use prometheus::{Encoder, Gauge, GaugeVec, IntCounter, IntCounterVec, Registry, TextEncoder};
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

// ---- per-asset metrics (label: asset) ----
static DEPTH_USD: Lazy<GaugeVec> = Lazy::new(|| {
    let g = GaugeVec::new(
        prometheus::Opts::new("depth_usd", "Bid depth in USD within 2% band"),
        &["asset"],
    )
    .unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static IMBALANCE: Lazy<GaugeVec> = Lazy::new(|| {
    let g = GaugeVec::new(
        prometheus::Opts::new("imbalance", "Order-book imbalance in 2% band [-1..1]"),
        &["asset"],
    )
    .unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static RISK_STATE: Lazy<GaugeVec> = Lazy::new(|| {
    let g = GaugeVec::new(
        prometheus::Opts::new("risk_state", "Risk state (0=OK,1=WARN,2=CRITICAL)"),
        &["asset"],
    )
    .unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static VOLUME_24H: Lazy<GaugeVec> = Lazy::new(|| {
    let g = GaugeVec::new(
        prometheus::Opts::new("volume_24h", "Trailing 24h traded volume USD"),
        &["asset"],
    )
    .unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static VWAP_24H: Lazy<GaugeVec> = Lazy::new(|| {
    let g = GaugeVec::new(prometheus::Opts::new("vwap_24h", "Trailing 24h VWAP"), &["asset"])
        .unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static TRADES_24H: Lazy<GaugeVec> = Lazy::new(|| {
    let g = GaugeVec::new(
        prometheus::Opts::new("trades_count_24h", "Trade count in trailing 24h"),
        &["asset"],
    )
    .unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static PENDING_UNLOCK_USD: Lazy<GaugeVec> = Lazy::new(|| {
    let g = GaugeVec::new(
        prometheus::Opts::new("pending_unlock_usd", "Next upcoming unlock priced in USD"),
        &["asset"],
    )
    .unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static ANOMALY_DEPTH_Z: Lazy<GaugeVec> = Lazy::new(|| {
    let g = GaugeVec::new(
        prometheus::Opts::new("anomaly_depth_z", "EWMA z-score of latest depth reading"),
        &["asset"],
    )
    .unwrap();
    MET_REGISTRY.register(Box::new(g.clone())).ok();
    g
});
static PAPER_SIGNALS: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new("paper_signals_total", "Paper signals by kind/result"),
        &["kind", "result"],
    )
    .unwrap();
    MET_REGISTRY.register(Box::new(c.clone())).ok();
    c
});
static OPEN_SIGNALS: Lazy<Gauge> = Lazy::new(|| {
    let g = Gauge::new("open_signals", "Currently open paper signals").unwrap();
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
#[allow(dead_code)]
pub fn set_depth_usd(asset: &str, v: f64) {
    DEPTH_USD.with_label_values(&[asset]).set(v);
}
#[allow(dead_code)]
pub fn set_imbalance(asset: &str, v: f64) {
    IMBALANCE.with_label_values(&[asset]).set(v);
}
#[allow(dead_code)]
pub fn set_risk_state(asset: &str, code: u8) {
    RISK_STATE.with_label_values(&[asset]).set(code as f64);
}
#[allow(dead_code)]
pub fn set_volume_24h(asset: &str, v: f64) {
    VOLUME_24H.with_label_values(&[asset]).set(v);
}
#[allow(dead_code)]
pub fn set_vwap_24h(asset: &str, v: f64) {
    VWAP_24H.with_label_values(&[asset]).set(v);
}
#[allow(dead_code)]
pub fn set_trades_count_24h(asset: &str, n: usize) {
    TRADES_24H.with_label_values(&[asset]).set(n as f64);
}
#[allow(dead_code)]
pub fn set_pending_unlock_usd(asset: &str, v: f64) {
    PENDING_UNLOCK_USD.with_label_values(&[asset]).set(v);
}
#[allow(dead_code)]
pub fn set_anomaly_z(asset: &str, z: Option<f64>) {
    ANOMALY_DEPTH_Z
        .with_label_values(&[asset])
        .set(z.unwrap_or(0.0));
}
#[allow(dead_code)]
pub fn inc_paper_signal(kind: &str, result: &str) {
    PAPER_SIGNALS.with_label_values(&[kind, result]).inc();
}
#[allow(dead_code)]
pub fn set_open_signals(n: usize) {
    OPEN_SIGNALS.set(n as f64);
}
