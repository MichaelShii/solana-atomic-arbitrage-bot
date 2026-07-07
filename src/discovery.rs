//! Proactive signature backscan — extract new mints from recent PumpSwap/DLMM transactions
//!
//! Periodically pulls recent transaction signatures from both programs via getSignaturesForAddress,
//! batch-fetches transaction logs, extracts mint candidates, and feeds them into the discovery channel.
//! This bypasses the WebSocket passive listening bottleneck (requires mint already in whitelist to pass filter).

use log::{debug, info};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc;

/// Pull once every 180s (non-critical path, reduced frequency to save RPC quota)
const BACKSCAN_INTERVAL: Duration = Duration::from_secs(180);
/// Number of signatures to pull per batch
const SIGNATURE_LIMIT: usize = 15;
use crate::constants::PUMPFUN_AMM_PROGRAM;
const DLMM_PROG: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";

/// Start background backscan task, returns join handle
pub fn spawn_backscan(rpc_url: String, discovery_tx: mpsc::UnboundedSender<String>) {
    tokio::spawn(async move {
        info!(
            "[BACKSCAN] started (interval={:?}, limit={})",
            BACKSCAN_INTERVAL, SIGNATURE_LIMIT
        );
        let mut seen_signatures: HashSet<String> = HashSet::new();
        let mut cycle: u64 = 0;

        loop {
            cycle += 1;
            let mut new_mints: u64 = 0;

            for (prog, _label) in [(PUMPFUN_AMM_PROGRAM, "PumpSwap"), (DLMM_PROG, "DLMM")] {
                let sigs = fetch_signatures(&rpc_url, prog, SIGNATURE_LIMIT).await;
                let fresh: Vec<String> = sigs
                    .into_iter()
                    .filter(|s| seen_signatures.insert(s.clone()))
                    .collect();

                if fresh.is_empty() {
                    continue;
                }

                // Batch-fetch transactions and extract mints
                for sig in fresh {
                    let mints = fetch_tx_mints(&rpc_url, &sig).await;
                    for m in mints {
                        // discovery channel handles dedup + verification
                        let _ = discovery_tx.send(m);
                        new_mints += 1;
                    }
                    // Small delay between getTransaction calls to avoid rate limit
                    tokio::time::sleep(Duration::from_millis(80)).await;
                }
            }

            // Prune dedup set periodically
            if seen_signatures.len() > 10_000 {
                seen_signatures.clear();
            }

            let total = seen_signatures.len();
            info!("[BACKSCAN] cycle={cycle} new_signatures_discovered={total} new_mints_fed={new_mints}");

            tokio::time::sleep(BACKSCAN_INTERVAL).await;
        }
    });
}

/// Call getSignaturesForAddress to retrieve recent signature list
async fn fetch_signatures(rpc_url: &str, program: &str, limit: usize) -> Vec<String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSignaturesForAddress",
        "params": [program, {"limit": limit}]
    });

    let resp: Value = {
        let result = client
            .post(rpc_url)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        match result {
            Ok(r) => match r.json().await {
                Ok(v) => v,
                Err(_) => {
                    debug!("[BACKSCAN] getSignaturesForAddress JSON parse failed for {program}");
                    return vec![];
                }
            },
            Err(_) => {
                debug!("[BACKSCAN] getSignaturesForAddress request failed for {program}");
                return vec![];
            }
        }
    };

    let sigs: Vec<String> = match resp.get("result") {
        Some(arr) => arr
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|entry| {
                        let sig = entry.get("signature")?.as_str()?;
                        let err = entry.get("err");
                        // Skip failed transactions
                        if err.is_some() && !err.unwrap().is_null() {
                            return None;
                        }
                        Some(sig.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default(),
        None => {
            debug!("[BACKSCAN] no result in getSignaturesForAddress response");
            return vec![];
        }
    };

    debug!("[BACKSCAN] fetched {} sigs from {}", sigs.len(), program);
    sigs
}

/// Fetch a single transaction and extract all pubkey candidates from it (base58, 32-44 chars)
async fn fetch_tx_mints(rpc_url: &str, sig: &str) -> Vec<String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [sig, {"encoding": "json", "maxSupportedTransactionVersion": 0}]
    });

    let resp: Value = {
        let result = client
            .post(rpc_url)
            .json(&body)
            .timeout(Duration::from_secs(8))
            .send()
            .await;
        match result {
            Ok(r) => match r.json().await {
                Ok(v) => v,
                Err(_) => return vec![],
            },
            Err(_) => return vec![],
        }
    };

    let result = match resp.get("result") {
        Some(r) => r,
        None => return vec![],
    };

    // Extract mints from account keys (outer message)
    let account_keys: Vec<&str> = result
        .get("transaction")
        .and_then(|tx| tx.get("message"))
        .and_then(|msg| msg.get("accountKeys"))
        .and_then(|arr| arr.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|ak| ak.as_str().or_else(|| ak.get("pubkey")?.as_str()))
                .collect()
        })
        .unwrap_or_default();

    // Also extract from log messages
    let log_candidates: Vec<String> = result
        .get("meta")
        .and_then(|m| m.get("logMessages"))
        .and_then(|logs| logs.as_array())
        .map(|logs| {
            logs.iter()
                .filter_map(|l| l.as_str())
                .flat_map(|log| {
                    log.split(|c: char| !c.is_alphanumeric())
                        .filter(|token| token.len() >= 32 && token.len() <= 44 && is_base58(token))
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default();

    // Combine and filter known non-mint addresses
    let known_filter: HashSet<&str> = [
        PUMPFUN_AMM_PROGRAM,
        DLMM_PROG,
        "So11111111111111111111111111111111111111112",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
        "11111111111111111111111111111111",
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
        "ComputeBudget111111111111111111111111111111",
        "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
        "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
    ]
    .into_iter()
    .collect();

    let all_candidates: Vec<String> = account_keys
        .into_iter()
        .map(String::from)
        .chain(log_candidates)
        .filter(|s| !known_filter.contains(s.as_str()) && is_base58(s))
        .collect();

    if all_candidates.is_empty() {
        return vec![];
    }

    // Dedup and return
    let mut seen = HashSet::new();
    all_candidates
        .into_iter()
        .filter(|m| seen.insert(m.clone()))
        .collect()
}

fn is_base58(s: &str) -> bool {
    s.bytes().all(|b| {
        matches!(
            b,
            b'1'..=b'9'
                | b'A'..=b'H'
                | b'J'..=b'N'
                | b'P'..=b'Z'
                | b'a'..=b'k'
                | b'm'..=b'z'
        )
    })
}
