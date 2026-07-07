//! Yellowstone gRPC real-time pool state cache
//!
//! Subscribes to PumpSwap + DLMM + CPMM + Whirlpool program account changes,
//! maintains a local DashMap cache. Scanning/build phases read from cache
//! instead of RPC queries, eliminating round-trip latency.

use dashmap::DashMap;
use log::{debug, info, warn};
use solana_sdk::pubkey::Pubkey;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_stream::StreamExt;
use yellowstone_grpc_client::{ClientTlsConfig, GeyserGrpcClient};
use yellowstone_grpc_proto::prelude::{
    SubscribeRequest, SubscribeRequestFilterAccounts,
};
use yellowstone_grpc_proto::geyser::subscribe_update::UpdateOneof;

static CACHE: OnceLock<GeyserPoolCache> = OnceLock::new();

/// Slot at startup — used as replay anchor for the first gRPC connection.
/// Set by main.rs after fetching the current slot from RPC.
pub static STARTUP_SLOT: AtomicU64 = AtomicU64::new(0);

pub fn global_cache() -> &'static GeyserPoolCache {
    CACHE.get_or_init(GeyserPoolCache::default)
}

/// gRPC account cache entry: (raw_data, slot, owner_pubkey)
struct CachedAccount {
    data: Vec<u8>,
    slot: u64,
    owner: Vec<u8>,
}

/// Global pool account cache: key = base58 pubkey, value = CachedAccount
pub struct GeyserPoolCache {
    accounts: DashMap<String, CachedAccount>,
    latest_slot: AtomicU64,
    /// Latest blockhash from gRPC SubscribeBlocksMeta (zero-RTT, no RPC needed).
    latest_blockhash: std::sync::RwLock<(solana_sdk::hash::Hash, u64)>,
}

impl Default for GeyserPoolCache {
    fn default() -> Self {
        Self {
            accounts: DashMap::new(),
            latest_slot: AtomicU64::new(0),
            latest_blockhash: std::sync::RwLock::new((solana_sdk::hash::Hash::default(), 0)),
        }
    }
}

impl GeyserPoolCache {
    pub fn get(&self, pubkey: &str) -> Option<Vec<u8>> {
        self.accounts.get(pubkey).map(|e| e.data.clone())
    }
    pub fn get_with_slot(&self, pubkey: &str) -> Option<(Vec<u8>, u64)> {
        self.accounts.get(pubkey).map(|e| (e.data.clone(), e.slot))
    }
    pub fn get_owner(&self, pubkey: &str) -> Option<Vec<u8>> {
        self.accounts.get(pubkey).map(|e| e.owner.clone())
    }
    pub fn put(&self, pubkey: String, data: Vec<u8>, owner: Vec<u8>, slot: u64) {
        self.latest_slot.fetch_max(slot, Ordering::Relaxed);
        self.accounts.insert(pubkey, CachedAccount { data, slot, owner });
    }
    pub fn latest_slot(&self) -> u64 {
        self.latest_slot.load(Ordering::Relaxed)
    }
    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    /// Update the cached blockhash. Called from gRPC BlockMeta updates.
    pub fn put_blockhash(&self, blockhash: solana_sdk::hash::Hash, slot: u64) {
        if let Ok(mut h) = self.latest_blockhash.write() {
            if slot > h.1 {
                *h = (blockhash, slot);
            }
        }
    }

    /// Get the latest blockhash from gRPC cache. Returns None if no blockhash yet.
    pub fn get_latest_blockhash(&self) -> Option<solana_sdk::hash::Hash> {
        self.latest_blockhash.read().ok().map(|h| h.0)
    }

    /// Get cached blockhash if it's not too stale. Returns None if missing or >5 slots old.
    pub fn get_fresh_blockhash(&self) -> Option<solana_sdk::hash::Hash> {
        let h = self.latest_blockhash.read().ok()?;
        let current = self.latest_slot.load(Ordering::Relaxed);
        // Blockhash is fresh if cached slot is within 10 slots of latest seen slot.
        if h.1 > 0 && current > 0 && current.saturating_sub(h.1) <= 10 {
            Some(h.0)
        } else {
            None
        }
    }
}

/// Start gRPC subscription background task — subscribes to PumpSwap / DLMM / CPMM / Whirlpool account changes
pub fn spawn_grpc_subscription(
    endpoint: String,
    x_token: String,
    programs: Vec<Pubkey>,
) {
    if endpoint.is_empty() || programs.is_empty() {
        info!("[GRPC] endpoint or programs not configured — skipping gRPC subscription");
        return;
    }
    // x_token can be empty for self-hosted nodes (localhost, no auth)

    // Initialize the global cache
    global_cache();

    tokio::spawn(async move {
        loop {
            info!("[GRPC] connecting to {endpoint}...");
            match connect_and_stream(&endpoint, &x_token, &programs).await {
                Ok(()) => {
                    // NLN silently closes stream when from_slot is invalid.
                    // Reset to 0 so next attempt uses live-only (no replay).
                    STARTUP_SLOT.store(0, Ordering::Relaxed);
                    info!("[GRPC] stream ended, reconnecting without replay...");
                }
                Err(e) => {
                    warn!("[GRPC] stream error: {e}, reconnecting in 5s...");
                    // If from_slot replay failed because the slot was pruned,
                    // reset the anchor so next attempt uses live-only.
                    let err_str = e.to_string();
                    if err_str.contains("past the valid range") || err_str.contains("broadcast from") || err_str.contains("failed to get replay position") {
                        STARTUP_SLOT.store(0, Ordering::Relaxed);
                        info!("[GRPC] from_slot pruned — switching to live-only");
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

async fn connect_and_stream(
    endpoint: &str,
    x_token: &str,
    programs: &[Pubkey],
) -> anyhow::Result<()> {
    let use_tls = endpoint.starts_with("https://");
    let mut builder = GeyserGrpcClient::build_from_shared(endpoint.to_string())?
        .x_token(if x_token.is_empty() { None } else { Some(x_token.to_string()) })?;
    if use_tls {
        builder = builder.tls_config(ClientTlsConfig::new().with_native_roots())?;
    }
    let mut client = builder
        .max_encoding_message_size(64 * 1024 * 1024)
        .connect()
        .await?;

    let version = client.get_version().await?;
    info!(
        "[GRPC] connected, version={version:?}, programs={}",
        programs.len()
    );

    // Replay from last known slot to bridge disconnection gaps.
    // Cold start: replay 1000 slots (~7 min) to warm the cache.
    // Warm restart: replay 500 slots to catch missed data during downtime.
    let replay_from = {
        let startup = STARTUP_SLOT.load(Ordering::Relaxed);
        if startup == 0 {
            None // pruned replay → start fresh
        } else {
            let latest = global_cache().latest_slot();
            if latest > 500 {
                Some(latest - 500)
            } else if startup > 1000 {
                Some(startup - 1000)
            } else {
                None
            }
        }
    };

    use solana_sdk::hash::Hash;
    use std::str::FromStr;
    use yellowstone_grpc_proto::prelude::SubscribeRequestFilterBlocksMeta;

    let request = SubscribeRequest {
        accounts: {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "".to_string(),
                SubscribeRequestFilterAccounts {
                    account: vec![],
                    owner: programs.iter().map(|p| p.to_string()).collect(),
                    filters: vec![],
                    nonempty_txn_signature: None,
                    cuckoo_accounts_filter: None,
                },
            );
            map
        },
        blocks_meta: {
            let mut map = std::collections::HashMap::new();
            map.insert("".to_string(), SubscribeRequestFilterBlocksMeta {});
            map
        },
        from_slot: replay_from,
        ..Default::default()
    };

    let mut stream = client.subscribe_once(request).await?;
    info!(
        "[GRPC] subscribed: accounts + blocksMeta, from_slot={:?}",
        replay_from
    );

    let cache = global_cache();
    while let Some(result) = stream.next().await {
        match result {
            Ok(update) => {
                match update.update_oneof {
                    Some(UpdateOneof::Account(acct)) => {
                        if let Some(info) = acct.account {
                            let pubkey_str = Pubkey::new_from_array(
                                info.pubkey.as_slice().try_into().unwrap_or([0; 32]),
                            )
                            .to_string();
                            cache.put(pubkey_str, info.data, info.owner, acct.slot);
                        }
                    }
                    Some(UpdateOneof::BlockMeta(meta)) => {
                        if !meta.blockhash.is_empty() {
                            if let Ok(hash) = Hash::from_str(&meta.blockhash) {
                                let is_first = cache.get_latest_blockhash().is_none();
                                cache.put_blockhash(hash, meta.slot);
                                if is_first {
                                    info!(
                                        "[GRPC] first blockhash cached: slot={} hash={}",
                                        meta.slot,
                                        &meta.blockhash[..12.min(meta.blockhash.len())]
                                    );
                                }
                            }
                        }
                    }
                    _ => {} // slots, transactions, etc. — not used
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("past the valid range") || msg.contains("broadcast from") {
                    STARTUP_SLOT.store(0, Ordering::Relaxed);
                    warn!("[GRPC] from_slot pruned — restarting without replay");
                    return Err(anyhow::anyhow!("from_slot pruned: {msg}"));
                }
                debug!("[GRPC] stream recv error: {e}");
            }
        }
    }

    Ok(())
}

// ── RabbitStream (shred-level transaction discovery) ───────────────

/// Launch a RabbitStream subscription to discover new token mints at
/// the shred layer, feeding them to the background discovery task.
pub fn spawn_rabbitstream(
    endpoint: String,
    x_token: String,
    programs: Vec<Pubkey>,
    discovery_tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    if endpoint.is_empty() || x_token.is_empty() || programs.is_empty() {
        info!("[RABBIT] endpoint or x_token not configured — skipping");
        return;
    }

    tokio::spawn(async move {
        let mut retry: u32 = 0;
        loop {
            if retry > 0 {
                let delay = std::time::Duration::from_secs((1 << retry.min(5)) as u64);
                warn!("[RABBIT] reconnecting in {}s (attempt {retry})...", delay.as_secs());
                tokio::time::sleep(delay).await;
            }
            info!("[RABBIT] connecting to {endpoint}...");
            match connect_rabbitstream(&endpoint, &x_token, &programs, &discovery_tx).await {
                Ok(()) => warn!("[RABBIT] stream ended, will reconnect..."),
                Err(e) => warn!("[RABBIT] error: {e}, will reconnect..."),
            }
            retry += 1;
        }
    });
}

async fn connect_rabbitstream(
    endpoint: &str,
    x_token: &str,
    programs: &[Pubkey],
    discovery_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> anyhow::Result<()> {
    use yellowstone_grpc_proto::prelude::SubscribeRequestFilterTransactions;
    use yellowstone_grpc_proto::geyser::CommitmentLevel;

    let mut client = GeyserGrpcClient::build_from_shared(endpoint.to_string())?
        .x_token(Some(x_token.to_string()))?
        .tls_config(ClientTlsConfig::new().with_native_roots())?
        .max_decoding_message_size(1024 * 1024 * 1024)
        .max_encoding_message_size(1024 * 1024 * 1024)
        .connect()
        .await?;

    // RabbitStream does not implement get_version; skip it gracefully.
    match client.get_version().await {
        Ok(v) => info!("[RABBIT] connected, version={v:?}"),
        Err(_) => info!("[RABBIT] connected (get_version not supported by RabbitStream)"),
    }

    let program_strs: Vec<String> = programs.iter().map(|p| p.to_string()).collect();

    let request = SubscribeRequest {
        transactions: {
            let mut map = std::collections::HashMap::new();
            map.insert(
                "rabbit".to_string(),
                SubscribeRequestFilterTransactions {
                    vote: Some(false),
                    failed: Some(false),
                    account_include: program_strs.clone(),
                    account_exclude: vec![],
                    account_required: vec![],
                    signature: None,
                    token_accounts: None,
                },
            );
            map
        },
        commitment: Some(CommitmentLevel::Processed as i32),
        ..Default::default()
    };

    // subscribe_with_request keeps the request channel open (sink held alive),
    // which RabbitStream requires. subscribe_once / stream::once close the
    // request stream immediately, causing RabbitStream to terminate the response.
    let (_sink, mut stream) = client.subscribe_with_request(Some(request)).await?;
    info!(
        "[RABBIT] subscribed, {} programs, shred-level discovery",
        programs.len()
    );

    // Known non-token programs / system addresses to skip.
    let skip_prefixes: &[&str] = &[
        "11111111111111111111111111111111", // System
        "ComputeBudget111111111111111111111111111111",
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
    ];

    let programs_set: std::collections::HashSet<String> =
        program_strs.into_iter().collect();

    while let Some(result) = stream.next().await {
        match result {
            Ok(update) => {
                if let Some(UpdateOneof::Transaction(tx_update)) = update.update_oneof {
                    if let Some(ref tx) = tx_update.transaction {
                        if let Some(msg) = tx.transaction.as_ref().and_then(|t| t.message.as_ref()) {
                            for key_bytes in &msg.account_keys {
                                if key_bytes.len() != 32 {
                                    continue;
                                }
                                let pk = Pubkey::new_from_array(
                                    key_bytes.as_slice().try_into().unwrap_or([0; 32]),
                                );
                                let pk_str = pk.to_string();
                                if skip_prefixes.contains(&pk_str.as_str())
                                    || programs_set.contains(&pk_str)
                                {
                                    continue;
                                }
                                let _ = discovery_tx.send(pk_str);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                debug!("[RABBIT] stream recv error: {e}");
            }
        }
    }

    Ok(())
}
