//! Dual WebSocket listener — subscribes to PumpSwap and DLMM separately
//!
//! Helius Free's logsSubscribe only supports Mentions with a single address.
//! Open two WSS connections, merge with SelectAll, achieving simultaneous
//! listening for both programs.
//!
//! Flow: receive swap → parse mint → check whitelist → query both-side prices → build atomic TX → submit

mod extract;
mod helpers;

use helpers::{determine_program, extract_mint_candidates, mask_url};

use crate::constants::NATIVE_SOL_MINT;
use crate::metrics::SharedMetrics;
use crate::pool_cache::cache_discovered_cpmm_pool;
use crate::pool_cache::cache_discovered_whirlpool_pool;
use crate::pool_cache::cache_dlmm_lb_pair;
use crate::pool_cache::{
    get_discovered_cpmm_pool, get_discovered_whirlpool_pool, get_dlmm_reserves,
    get_whirlpool_reserves,
};
use crate::whitelist::Whitelist;
use futures::stream::SelectAll;
use futures::StreamExt;
use log::{info, warn};
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::rpc_config::{RpcTransactionLogsConfig, RpcTransactionLogsFilter};
use solana_sdk::commitment_config::CommitmentConfig;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

/// Swap event — trigger for event-driven arbitrage
#[derive(Debug, Clone)]
pub struct SwapEvent {
    /// The meme token mint involved (extracted from transaction logs)
    pub mint: Option<String>,
}

/// Start dual WebSocket listening, returning a swap event receiver.
///
/// Opens two WSS connections:
/// - One subscribes to PumpSwap (pAMMBay6oceH)
/// - One subscribes to DLMM (LBUZKhRx)
///   Merged and sent upstream via mpsc::Sender.
pub async fn run_dual_listen(
    ws_url: &str,
    rpc_url: &str,
    whitelist: Arc<RwLock<Whitelist>>,
    discovery_tx: mpsc::UnboundedSender<String>,
    metrics: SharedMetrics,
) -> anyhow::Result<mpsc::Receiver<SwapEvent>> {
    let (tx, rx) = mpsc::channel(4096);
    let ws_url = ws_url.to_string();
    let rpc_url = rpc_url.to_string();

    tokio::spawn(async move {
        let mut retry: u32 = 0;
        loop {
            if retry > 0 {
                let delay = Duration::from_secs((1 << retry.min(5)) as u64);
                warn!(
                    "[LISTENER] reconnecting in {}s (attempt {retry})...",
                    delay.as_secs()
                );
                tokio::time::sleep(delay).await;
            }
            match dual_listen_loop(
                &ws_url,
                &rpc_url,
                &tx,
                whitelist.clone(),
                discovery_tx.clone(),
                metrics.clone(),
            )
            .await
            {
                Ok(()) => warn!("[LISTENER] stream ended, will reconnect..."),
                Err(e) => warn!("[LISTENER] error: {e}, will reconnect..."),
            }
            retry += 1;
        }
    });

    Ok(rx)
}

async fn dual_listen_loop(
    ws_url: &str,
    rpc_url: &str,
    tx: &mpsc::Sender<SwapEvent>,
    whitelist: Arc<RwLock<Whitelist>>,
    discovery_tx: mpsc::UnboundedSender<String>,
    metrics: SharedMetrics,
) -> anyhow::Result<()> {
    info!("[LISTENER] connecting to {}", mask_url(ws_url));
    let client = PubsubClient::new(ws_url)
        .await
        .map_err(|e| anyhow::anyhow!("PubsubClient: {e}"))?;

    let log_conf = RpcTransactionLogsConfig {
        commitment: Some(CommitmentConfig::processed()),
    };

    let pump_prog = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
    let dlmm_prog = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
    // Shyft WS supports 4+ subscriptions without connection drops
    let cpmm_prog = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";
    let whirlpool_prog = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

    let mut merged = SelectAll::new();

    // Subscribe to PumpSwap
    match client
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(vec![pump_prog.to_string()]),
            log_conf.clone(),
        )
        .await
    {
        Ok((stream, _unsub)) => {
            info!("[LISTENER] subscribed to PumpSwap");
            merged.push(stream);
        }
        Err(e) => warn!("[LISTENER] PumpSwap subscription failed: {e}"),
    }

    // Subscribe to DLMM (reuses the same client; logs_subscribe takes &self and supports multiple subscriptions)
    match client
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(vec![dlmm_prog.to_string()]),
            log_conf.clone(),
        )
        .await
    {
        Ok((stream, _unsub)) => {
            info!("[LISTENER] subscribed to DLMM");
            merged.push(stream);
        }
        Err(e) => warn!("[LISTENER] DLMM subscription failed: {e}"),
    }

    // Subscribe to CPMM
    match client
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(vec![cpmm_prog.to_string()]),
            log_conf.clone(),
        )
        .await
    {
        Ok((stream, _unsub)) => {
            info!("[LISTENER] subscribed to CPMM");
            merged.push(stream);
        }
        Err(e) => warn!("[LISTENER] CPMM subscription failed: {e}"),
    }

    // Subscribe to Orca Whirlpool
    match client
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(vec![whirlpool_prog.to_string()]),
            log_conf,
        )
        .await
    {
        Ok((stream, _unsub)) => {
            info!("[LISTENER] subscribed to Orca Whirlpool");
            merged.push(stream);
        }
        Err(e) => warn!("[LISTENER] Orca Whirlpool subscription failed: {e}"),
    }

    // slot_subscribe heartbeat: pushes every slot (~400ms), prevents Helius 10min inactivity disconnect
    // Keep _unsub handle to maintain the subscription; _stream is dropped but the server continues pushing to keep the connection alive
    let _slot_unsub = match client.slot_subscribe().await {
        Ok((_stream, unsub)) => {
            info!("[LISTENER] slot heartbeat active");
            Some(unsub)
        }
        Err(e) => {
            warn!("[LISTENER] slot_subscribe failed: {e}");
            None
        }
    };

    if merged.is_empty() {
        anyhow::bail!("all subscriptions failed");
    }

    info!("[LISTENER] dual listen active (PumpSwap + DLMM)");
    let mut count: u64 = 0;
    let mut filtered: u64 = 0;
    let mut by_program: HashMap<String, u64> = HashMap::new();
    let mut seen_mints: HashSet<String> = HashSet::new();
    let mut processed_sigs: HashSet<String> = HashSet::with_capacity(256);

    while let Some(response) = merged.next().await {
        // Skip failed transactions
        if response.value.err.is_some() {
            continue;
        }

        // Log heuristic filtering
        let has_trade = response
            .value
            .logs
            .iter()
            .any(|l| l.contains("Swap") || l.contains("Buy") || l.contains("Sell"));
        if !has_trade {
            continue;
        }

        // Extract candidate pubkeys from logs → whitelist filter (no RPC call needed)
        let candidates = extract_mint_candidates(&response.value.logs);
        let wl = whitelist.read().await;
        let mint_matched = candidates.iter().find(|c| wl.contains(c));
        let mint_str = match mint_matched {
            Some(m) => m.clone(),
            None => {
            filtered += 1;
            metrics.events_filtered.inc();
            // Feed to discovery channel: newly seen candidate mints sent for background batch validation
            for c in &candidates {
                if seen_mints.insert(c.clone()) {
                    let _ = discovery_tx.send(c.clone());
                }
            }
            // Periodically clear the dedup set to avoid unbounded memory growth
            if seen_mints.len() > 10000 {
                seen_mints.clear();
            }
            continue;
        }
        };
        let mint = Some(mint_str.clone());

        let program = determine_program(&response.value.logs);
        *by_program.entry(program.clone()).or_insert(0) += 1;
        match program.as_str() {
            "pumpfun" => metrics.events_pumpfun.inc(),
            "dlmm" => metrics.events_dlmm.inc(),
            "cpmm" => metrics.events_cpmm.inc(),
            "whirlpool" => metrics.events_whirlpool.inc(),
            _ => {}
        }

        let event = SwapEvent {
            mint,
        };

        match tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {}
        }

        // DLMM event: spawn background fetch to extract lb_pair for caching.
        // Skip getTransaction if we already have pool data for (SOL, mint) cached.
        if program == "dlmm" && get_dlmm_reserves(NATIVE_SOL_MINT, &mint_str).is_none() {
            let sig = response.value.signature.clone();
            if processed_sigs.insert(sig.clone()) {
                let rpc_url = rpc_url.to_string();
                let discovery = discovery_tx.clone();
                tokio::spawn(async move {
                    if let Some((mint_a, mint_b, lb_pair)) =
                        extract::extract_lb_pair_from_tx(&rpc_url, &sig).await
                    {
                        cache_dlmm_lb_pair(&mint_a, &mint_b, &lb_pair);
                        let _ = discovery.send(mint_a);
                        let _ = discovery.send(mint_b);
                    }
                });
            }
            if processed_sigs.len() > 1024 {
                processed_sigs.clear();
            }
        }

        // CPMM event: skip if pool already discovered from SOL pair.
        if program == "cpmm" && get_discovered_cpmm_pool(NATIVE_SOL_MINT, &mint_str).is_none() {
            let sig = response.value.signature.clone();
            if processed_sigs.insert(sig.clone()) {
                let rpc_url = rpc_url.to_string();
                let discovery = discovery_tx.clone();
                tokio::spawn(async move {
                    if let Some((mint_a, mint_b, pool_addr)) =
                        extract::extract_cpmm_pool_from_tx(&rpc_url, &sig).await
                    {
                        cache_discovered_cpmm_pool(&mint_a, &mint_b, &pool_addr);
                        let _ = discovery.send(mint_a);
                        let _ = discovery.send(mint_b);
                    }
                });
            }
        }

        // Whirlpool event: skip if pool already cached or discovered.
        if program == "whirlpool"
            && get_discovered_whirlpool_pool(NATIVE_SOL_MINT, &mint_str).is_none()
            && get_whirlpool_reserves(NATIVE_SOL_MINT, &mint_str).is_none()
        {
            let sig = response.value.signature.clone();
            if processed_sigs.insert(sig.clone()) {
                let rpc_url = rpc_url.to_string();
                let discovery = discovery_tx.clone();
                tokio::spawn(async move {
                    if let Some((mint_a, mint_b, pool_addr)) =
                        extract::extract_whirlpool_from_tx(&rpc_url, &sig).await
                    {
                        cache_discovered_whirlpool_pool(&mint_a, &mint_b, &pool_addr);
                        let _ = discovery.send(mint_a);
                        let _ = discovery.send(mint_b);
                    }
                });
            }
        }

        count += 1;
        if count.is_multiple_of(100) {
            info!(
                "[LISTENER] {} events ({} filtered) | programs: {:?}",
                count, filtered, by_program,
            );
        }
    }

    info!("[LISTENER] stream ended after {} events", count);
    Ok(())
}
