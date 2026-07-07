//! Multi-endpoint latency benchmark: WS + RPC (local vs Shyft vs Helius).
//! Run: cargo run --release --bin bench_latency
//!
//! Measures:
//!   WS — first-arrival delta per transaction signature across 3 sources
//!   RPC — round-trip time for getSlot / getLatestBlockhash

use dashmap::DashMap;
use log::{info, warn};
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::{RpcTransactionLogsConfig, RpcTransactionLogsFilter};
use solana_sdk::commitment_config::CommitmentConfig;
use std::sync::Arc;
use std::time::Instant;
use tokio_stream::StreamExt;
use tracing_subscriber::prelude::*;

// ── Config ──

struct WsEndpoint { name: &'static str, url: &'static str }
struct RpcEndpoint { name: &'static str, url: &'static str }

const WS_ENDPOINTS: &[WsEndpoint] = &[
    WsEndpoint { name: "local",    url: "ws://127.0.0.1:8900" },
    WsEndpoint { name: "shyft-ny", url: "wss://rpc.example.com?api_key=YOUR_API_KEY" },
    WsEndpoint { name: "helius",   url: "wss://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_API_KEY" },
];

const RPC_ENDPOINTS: &[RpcEndpoint] = &[
    RpcEndpoint { name: "local",   url: "http://127.0.0.1:8899" },
    RpcEndpoint { name: "shyft-ny", url: "https://rpc.example.com?api_key=YOUR_API_KEY" },
    RpcEndpoint { name: "helius",  url: "https://mainnet.helius-rpc.com/?api-key=YOUR_HELIUS_API_KEY" },
];

const WS_FILTER: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"; // PumpSwap

// ── WS Signal Tracker ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct WsHit {
    source: &'static str,
    ts: Instant,
}

struct WsTracker {
    map: DashMap<String, Vec<WsHit>>,
    counts: DashMap<&'static str, u64>,
    matches: DashMap<&'static str, u64>,  // "source vs source" → count
}

impl WsTracker {
    fn new() -> Self {
        Self {
            map: DashMap::new(),
            counts: DashMap::new(),
            matches: DashMap::new(),
        }
    }

    fn record(&self, sig: &str, source: &'static str) {
        let now = Instant::now();
        *self.counts.entry(source).or_insert(0) += 1;

        let mut hits = self.map.entry(sig.to_string()).or_insert_with(Vec::new);
        // Check if this source already recorded for this sig
        if hits.iter().any(|h| h.source == source) {
            return;
        }
        hits.push(WsHit { source, ts: now });

        if hits.len() >= 2 {
            // Compare all pairs
            for i in 0..hits.len() {
                for j in (i+1)..hits.len() {
                    let first = &hits[i];
                    let second = &hits[j];
                    let (faster, slower, delta_ms) = if first.ts < second.ts {
                        (first.source, second.source, second.ts.duration_since(first.ts).as_millis() as i64)
                    } else {
                        (second.source, first.source, first.ts.duration_since(second.ts).as_millis() as i64)
                    };
                    let key = format!("{faster}>{slower}");
                    *self.matches.entry(Box::leak(key.into_boxed_str())).or_insert(0) += 1;
                    let key2 = format!("{faster}_ms");
                    // Accumulate min/max via separate counters
                    info!(
                        "[WS MATCH] sig={} {faster} ahead of {slower} by {delta_ms}ms",
                        &sig[..12.min(sig.len())],
                    );
                }
            }
        }
    }

    fn stats(&self) {
        info!("=== WS Stats ===");
        for e in WS_ENDPOINTS {
            let c = self.counts.get(e.name).map(|v| *v).unwrap_or(0);
            info!("  {} events: {c}", e.name);
        }
        // Show pair comparisons
        for entry in self.matches.iter() {
            let key = entry.key();
            let val = entry.value();
            if key.ends_with("_ms") { continue; }
            let count_key = format!("{key}_ms");
            info!("  head-to-head {key}: {val} times");
        }
    }
}

// ── RPC Bench ──

const RPC_SAMPLES: usize = 20;

async fn rpc_bench_round(ep: &RpcEndpoint) {
    let rpc = RpcClient::new_with_timeout(ep.url.to_string(), std::time::Duration::from_secs(10));

    // ── getSlot ──
    let mut slots = Vec::with_capacity(RPC_SAMPLES);
    for _ in 0..RPC_SAMPLES {
        let t0 = Instant::now();
        match rpc.get_slot().await {
            Ok(s) => slots.push((t0.elapsed().as_millis() as u64, s)),
            Err(_) => {}
        }
    }
    if !slots.is_empty() {
        let min = slots.iter().map(|(d,_)| *d).min().unwrap();
        let max = slots.iter().map(|(d,_)| *d).max().unwrap();
        let avg = slots.iter().map(|(d,_)| *d).sum::<u64>() / slots.len() as u64;
        let slot = slots[0].1;
        info!("[RPC {:<6}] getSlot x{}: slot={slot} min={min}ms avg={avg}ms max={max}ms", ep.name, slots.len());
    }

    // ── getLatestBlockhash ──
    let mut times = Vec::with_capacity(RPC_SAMPLES);
    for _ in 0..RPC_SAMPLES {
        let t0 = Instant::now();
        match rpc.get_latest_blockhash().await {
            Ok(_) => times.push(t0.elapsed().as_millis() as u64),
            Err(_) => {}
        }
    }
    if !times.is_empty() {
        let min = *times.iter().min().unwrap();
        let max = *times.iter().max().unwrap();
        let avg = times.iter().sum::<u64>() / times.len() as u64;
        info!("[RPC {:<6}] getBlockhash x{}: min={min}ms avg={avg}ms max={max}ms", ep.name, times.len());
    }
}

// ── Main ──

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();
    tracing_log::LogTracer::init().ok();

    let tracker = Arc::new(WsTracker::new());

    // ── WS subscriptions ──
    for ep in WS_ENDPOINTS {
        let t = tracker.clone();
        let name = ep.name;
        let url = ep.url.to_string();
        tokio::spawn(async move {
            loop {
                info!("[WS {name}] connecting...");
                match ws_listen(&url, name, &t).await {
                    Ok(()) => warn!("[WS {name}] ended"),
                    Err(e) => warn!("[WS {name}] error: {e}"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        });
    }

    // ── RPC bench loop ──
    tokio::spawn(async move {
        loop {
            info!("=== RPC round ===");
            for ep in RPC_ENDPOINTS {
                rpc_bench_round(ep).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });

    // ── Stats reporter ──
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        tracker.stats();
    }
}

// ── WS listener ──

async fn ws_listen(
    url: &str,
    name: &'static str,
    tracker: &WsTracker,
) -> anyhow::Result<()> {
    let client = PubsubClient::new(url).await?;
    let log_conf = RpcTransactionLogsConfig {
        commitment: Some(CommitmentConfig::processed()),
    };
    let (mut stream, _unsub) = client
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(vec![WS_FILTER.to_string()]),
            log_conf,
        )
        .await?;
    info!("[WS {name}] subscribed (PumpSwap)");

    while let Some(response) = stream.next().await {
        if response.value.err.is_some() {
            continue;
        }
        tracker.record(&response.value.signature, name);
    }
    Ok(())
}
