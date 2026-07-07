//! Standalone signal race: gRPC vs WebSocket latency measurement.
//! Run: cargo run --release --bin signal_race
//!
//! No bot main loop, no price scans, no TX building — pure timing.

use log::{debug, info, warn};
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::rpc_config::{RpcTransactionLogsConfig, RpcTransactionLogsFilter};
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_stream::StreamExt;
use tracing_subscriber::prelude::*;

use yellowstone_grpc_client::GeyserGrpcClient;
use yellowstone_grpc_proto::geyser::subscribe_update::UpdateOneof;
use yellowstone_grpc_proto::prelude::{
    SubscribeRequest, SubscribeRequestFilterTransactions,
};

// ── Signal map ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Source {
    Grpc,
    Ws,
}

struct SignalTracker {
    map: Mutex<HashMap<String, (Source, Instant)>>,
    grpc_count: std::sync::atomic::AtomicU64,
    ws_count: std::sync::atomic::AtomicU64,
    match_count: std::sync::atomic::AtomicU64,
}

impl SignalTracker {
    fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
            grpc_count: std::sync::atomic::AtomicU64::new(0),
            ws_count: std::sync::atomic::AtomicU64::new(0),
            match_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn record(&self, sig: &str, source: Source) {
        let now = Instant::now();
        match source {
            Source::Grpc => { self.grpc_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
            Source::Ws => { self.ws_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
        }
        let mut map = self.map.lock().unwrap();
        if let Some((prev_src, prev_time)) = map.get(sig) {
            if *prev_src != source {
                let n = self.match_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let delta_ms = prev_time.elapsed().as_millis() as i64;
                let first = match *prev_src { Source::Grpc => "GRPC", Source::Ws => "WS" };
                let second = match source { Source::Grpc => "GRPC", Source::Ws => "WS" };
                info!(
                    "[MATCH #{n}] sig={} first={} ahead={}ms ({} was {}ms behind)",
                    &sig[..12.min(sig.len())],
                    first,
                    delta_ms,
                    second,
                    delta_ms,
                );
                if n >= 99 {
                    info!("[DONE] 100 matches collected, exiting");
                    std::process::exit(0);
                }
            }
        } else {
            map.insert(sig.to_string(), (source, now));
            if map.len() > 500_000 {
                map.clear();
            }
        }
    }
}

// ── Config ──

const GRPC_URL: &str = "http://127.0.0.1:10000";
const WS_URL: &str = "wss://rpc.example.com";  // or use Shyft WS
const WS_API_KEY: &str = env!("SHYFT_API_KEY");

// ── Main ──

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();
    tracing_log::LogTracer::init().ok();

    let programs = vec![
        Pubkey::from_str("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA")?,   // PumpSwap
        Pubkey::from_str("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo")?,   // DLMM
        Pubkey::from_str("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C")?,   // CPMM
        Pubkey::from_str("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc")?,   // Whirlpool
    ];

    let tracker = Arc::new(SignalTracker::new());

    // ── gRPC subscription ──
    let tracker_g = tracker.clone();
    let programs_g = programs.clone();
    tokio::spawn(async move {
        loop {
            info!("[GRPC] connecting to {GRPC_URL}...");
            match grpc_subscribe(GRPC_URL, &programs_g, &tracker_g).await {
                Ok(()) => warn!("[GRPC] stream ended"),
                Err(e) => warn!("[GRPC] error: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    });

    // ── WebSocket subscription ──
    let tracker_w = tracker.clone();
    tokio::spawn(async move {
        let ws_url = format!("{WS_URL}?api_key={WS_API_KEY}");
        loop {
            info!("[WS] connecting...");
            match ws_subscribe(&ws_url, &programs, &tracker_w).await {
                Ok(()) => warn!("[WS] stream ended"),
                Err(e) => warn!("[WS] error: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    });

    // ── Status reporter ──
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        info!(
            "[STATS] grpc={} ws={} matches={} map={}",
            tracker.grpc_count.load(std::sync::atomic::Ordering::Relaxed),
            tracker.ws_count.load(std::sync::atomic::Ordering::Relaxed),
            tracker.match_count.load(std::sync::atomic::Ordering::Relaxed),
            tracker.map.lock().unwrap().len(),
        );
    }
}

// ── gRPC stream ──

async fn grpc_subscribe(
    endpoint: &str,
    programs: &[Pubkey],
    tracker: &SignalTracker,
) -> anyhow::Result<()> {
    let mut client = GeyserGrpcClient::build_from_shared(endpoint.to_string())?
        .x_token::<String>(None)?
        .max_encoding_message_size(64 * 1024 * 1024)
        .connect()
        .await?;

    let program_strs: Vec<String> = programs.iter().map(|p| p.to_string()).collect();
    let request = SubscribeRequest {
        transactions: {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "signal_race".to_string(),
                SubscribeRequestFilterTransactions {
                    vote: Some(false),
                    failed: Some(false),
                    account_include: program_strs,
                    account_exclude: vec![],
                    account_required: vec![],
                    signature: None,
                    token_accounts: None,
                },
            );
            map
        },
        ..Default::default()
    };

    // Use raw tonic subscribe to avoid subscribe_once's AutoReconnect/DedupStream buffering
    let request_stream = futures::stream::once(async { request });
    let response = client.geyser.subscribe(request_stream).await?;
    let mut stream = response.into_inner();
    info!("[GRPC] subscribed to transactions (raw)");

    while let Some(result) = stream.next().await {
        match result {
            Ok(update) => {
                if let Some(UpdateOneof::Transaction(tx)) = update.update_oneof {
                    if let Some(ref info) = tx.transaction {
                        let sig = solana_sdk::bs58::encode(&info.signature).into_string();
                        tracker.record(&sig, Source::Grpc);
                    }
                }
            }
            Err(e) => debug!("[GRPC] recv err: {e}"),
        }
    }
    Ok(())
}

// ── WebSocket stream ──

async fn ws_subscribe(
    ws_url: &str,
    programs: &[Pubkey],
    tracker: &SignalTracker,
) -> anyhow::Result<()> {
    let client = PubsubClient::new(ws_url).await?;
    let log_conf = RpcTransactionLogsConfig {
        commitment: Some(CommitmentConfig::processed()),
    };

    let mut merged = futures::stream::SelectAll::new();

    for prog in programs {
        match client
            .logs_subscribe(
                RpcTransactionLogsFilter::Mentions(vec![prog.to_string()]),
                log_conf.clone(),
            )
            .await
        {
            Ok((stream, _unsub)) => {
                info!("[WS] subscribed to {}", &prog.to_string()[..8.min(prog.to_string().len())]);
                merged.push(stream);
            }
            Err(e) => warn!("[WS] subscribe failed for {}: {e}", prog),
        }
    }

    if merged.is_empty() {
        anyhow::bail!("all WS subscriptions failed");
    }

    info!("[WS] listening ({})", merged.len());

    while let Some(response) = merged.next().await {
        if response.value.err.is_some() {
            continue;
        }
        let sig = response.value.signature;
        tracker.record(&sig, Source::Ws);
    }
    Ok(())
}
