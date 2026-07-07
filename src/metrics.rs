//! Prometheus metrics endpoint
//!
//! Exposes operational metrics on config.monitoring.metrics_port via HTTP.

use prometheus::{Counter, Encoder, Gauge, Registry, TextEncoder};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

pub struct Metrics {
    pub registry: Registry,
    pub opportunities_scanned: Counter,
    pub submissions_failed: Counter,
    pub trades_succeeded: Counter,
    pub profit_sol: Gauge,
    #[allow(dead_code)]
    pub circuit_breaker: Gauge,
    // Per-program event counters
    pub events_pumpfun: Counter,
    pub events_dlmm: Counter,
    pub events_cpmm: Counter,
    pub events_whirlpool: Counter,
    pub events_filtered: Counter,
    // Discovery
    pub whitelist_size: Gauge,
    pub discovered_mints: Counter,
    pub confirmations_late: Counter,
}

pub type SharedMetrics = Arc<Metrics>;

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let mk_counter = |name: &str, help: &str| -> Counter {
            let c = Counter::new(name, help).unwrap();
            registry.register(Box::new(c.clone())).unwrap();
            c
        };
        let mk_gauge = |name: &str, help: &str| -> Gauge {
            let g = Gauge::new(name, help).unwrap();
            registry.register(Box::new(g.clone())).unwrap();
            g
        };

        Self {
            opportunities_scanned: mk_counter(
                "mevbot_opportunities_scanned_total",
                "Total arbitrage opportunities found",
            ),
            submissions_failed: mk_counter(
                "mevbot_submissions_failed_total",
                "Total TX submissions that failed",
            ),
            trades_succeeded: mk_counter(
                "mevbot_trades_succeeded_total",
                "Total trades that landed on-chain",
            ),
            profit_sol: mk_gauge("mevbot_profit_sol", "Cumulative net profit in SOL"),
            circuit_breaker: mk_gauge(
                "mevbot_circuit_breaker",
                "1 if circuit breaker is tripped, 0 otherwise",
            ),
            events_pumpfun: mk_counter("mevbot_events_pumpfun_total", "Swap events from Pump.fun"),
            events_dlmm: mk_counter("mevbot_events_dlmm_total", "Swap events from Meteora DLMM"),
            events_cpmm: mk_counter("mevbot_events_cpmm_total", "Swap events from Raydium CPMM"),
            events_whirlpool: mk_counter(
                "mevbot_events_whirlpool_total",
                "Swap events from Orca Whirlpool",
            ),
            events_filtered: mk_counter(
                "mevbot_events_filtered_total",
                "Events filtered out (non-matching mint or non-trade)",
            ),
            whitelist_size: mk_gauge("mevbot_whitelist_size", "Number of mints in the whitelist"),
            discovered_mints: mk_counter(
                "mevbot_discovered_mints_total",
                "New mints added to whitelist via discovery",
            ),
            confirmations_late: mk_counter(
                "mevbot_confirmations_late_total",
                "Confirmations that took >30s from submission",
            ),
            registry,
        }
    }
}

/// Start a minimal HTTP server serving `/metrics` in Prometheus text format.
pub fn start_metrics_server(metrics: SharedMetrics, port: u16) {
    tokio::spawn(async move {
        let addr = format!("0.0.0.0:{port}");
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                log::warn!("[METRICS] failed to bind {addr}: {e}");
                return;
            }
        };
        log::info!("[METRICS] serving /metrics on :{port}");

        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let m = metrics.clone();
                    tokio::spawn(async move {
                        let encoder = TextEncoder::new();
                        let metric_families = m.registry.gather();
                        let mut buf = Vec::new();
                        if encoder.encode(&metric_families, &mut buf).is_err() {
                            return;
                        }
                        let body = String::from_utf8_lossy(&buf);
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(),
                            body,
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
                Err(e) => {
                    log::error!("[METRICS] accept error: {e}");
                }
            }
        }
    });
}
