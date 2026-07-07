//! CPMM pool cache fetching
//!
//! PDA derivation of pool address, parse borsh PoolState, read vault balances.

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::{
    cache_key, compute_liquidity_sol, parse_token_amount, CacheEntry, PoolReserves,
    PoolStateData, CPMM_CACHE,
};
use crate::constants::{CPMM_AMM_CONFIG, CPMM_PROGRAM};
const TOKEN0_MINT_OFFSET: usize = 168;
const TOKEN1_MINT_OFFSET: usize = 200;

/// Load persisted CPMM pool addresses into cache at startup.
pub fn load_cpmm_pools() {
    let rows = crate::persistence::cpmm_pools_load_all();
    let mut cache = CPMM_CACHE.write().unwrap_or_else(|e| {
        log::warn!("CPMM cache poisoned, recovering");
        e.into_inner()
    });
    for (mint_a, mint_b, pool_addr, config_addr) in &rows {
        let key = cache_key(mint_a, mint_b);
        if !cache.contains_key(&key) {
            let state = PoolStateData {
                pool_pubkey: pool_addr.clone(),
                config_pubkey: config_addr.clone(),
                token_0_mint: mint_a.clone(),
                token_1_mint: mint_b.clone(),
                token_0_vault: String::new(),
                token_1_vault: String::new(),
                token_0_vault_raw: 0,
                token_1_vault_raw: 0,
            };
            cache.insert(key, CacheEntry::new_persisted(Some(state)));
        }
    }
    log::info!("[CPMM] loaded {} pools from DB", rows.len());
}

/// Sync fetch + write cache, return SOL-equivalent liquidity (CPMM)
pub async fn fetch_cpmm_now(rpc: &RpcClient, t0: &str, t1: &str) -> Option<f64> {
    let key = cache_key(t0, t1);
    if let Some(entry) = CPMM_CACHE.read().ok()?.get(&key) {
        if !entry.is_stale() {
            return entry
                .data
                .as_ref()
                .and_then(|s| compute_liquidity_sol(&PoolReserves::from_state(s)));
        }
    }

    let state = fetch_cpmm_pool(rpc, t0, t1).await;
    let liq = state
        .as_ref()
        .and_then(|s| compute_liquidity_sol(&PoolReserves::from_state(s)));
    let mut cache = CPMM_CACHE.write().unwrap_or_else(|e| {
        log::warn!("CPMM cache poisoned, recovering");
        e.into_inner()
    });
    cache.insert(key, super::CacheEntry::new(state));
    liq
}

/// Directly scan and read CPMM pool state from transaction account_keys (bypass PDA derivation)
pub async fn fetch_cpmm_by_address(rpc: &RpcClient, pool_addr: &str) -> Option<PoolStateData> {
    let key = format!("addr:{pool_addr}");
    if let Some(entry) = CPMM_CACHE.read().ok()?.get(&key) {
        if !entry.is_stale() {
            return entry.data.clone();
        }
    }

    let state = read_pool_state_at(rpc, pool_addr).await;
    let mut cache = CPMM_CACHE.write().unwrap_or_else(|e| {
        log::warn!("CPMM cache poisoned, recovering");
        e.into_inner()
    });
    cache.insert(key, super::CacheEntry::new(state.clone()));
    if let Some(ref s) = state {
        let mint_key = cache_key(&s.token_0_mint, &s.token_1_mint);
        let should_insert = match cache.get(&mint_key) {
            None => true,
            Some(entry) if entry.data.is_none() => true,
            _ => false,
        };
        if should_insert {
            cache.insert(mint_key, super::CacheEntry::new(Some(s.clone())));
        }
    }
    state
}

/// Scan and find CPMM pool state from transaction accounts JSON list, read and cache
#[allow(dead_code)]
pub async fn scan_and_fetch_cpmm(rpc: &RpcClient, accounts_json: &str) -> Option<f64> {
    let accts: Vec<String> = serde_json::from_str(accounts_json).ok()?;
    let cpmm_pk = Pubkey::from_str(CPMM_PROGRAM).ok()?;

    let uncached: Vec<&str> = accts
        .iter()
        .filter(|a| {
            let key = format!("addr:{a}");
            CPMM_CACHE
                .read()
                .ok()
                .map(|c| !c.contains_key(&key))
                .unwrap_or(true)
        })
        .map(|s| s.as_str())
        .collect();

    if uncached.is_empty() {
        return None;
    }

    let pks: Vec<Pubkey> = uncached
        .iter()
        .filter_map(|s| Pubkey::from_str(s).ok())
        .collect();
    let resp = rpc.get_multiple_accounts(&pks).await.ok()?;
    for (i, acct_opt) in resp.iter().enumerate() {
        let acct = acct_opt.as_ref()?;
        if acct.owner != cpmm_pk {
            continue;
        }
        if acct.data.len() < 240 {
            continue;
        }
        let pool_addr = uncached[i];
        if let Some(state) = fetch_cpmm_by_address(rpc, pool_addr).await {
            log::info!(
                "[CPMM] pool discovered from accounts: {} mint0={} mint1={}",
                &pool_addr[..12.min(pool_addr.len())],
                &state.token_0_mint[..12.min(state.token_0_mint.len())],
                &state.token_1_mint[..12.min(state.token_1_mint.len())],
            );
            let liq = super::get_reserves_sol(&state.token_0_mint, &state.token_1_mint)
                .map(|(a, b)| (a + b).max(0.0));
            return liq;
        }
    }
    None
}

// ============================================================
// Internal implementation
// ============================================================

async fn fetch_cpmm_pool(rpc: &RpcClient, t0: &str, t1: &str) -> Option<PoolStateData> {
    let cpmm = Pubkey::from_str(CPMM_PROGRAM).ok()?;

    let (token_0, token_1) = if t0 < t1 { (t0, t1) } else { (t1, t0) };
    let t0_pk = Pubkey::from_str(token_0).ok()?;
    let t1_pk = Pubkey::from_str(token_1).ok()?;

    // Most pools use config index 0 (D4FPEru...). Try it first.
    let config0 = Pubkey::from_str(CPMM_AMM_CONFIG).ok()?;
    let (pool0, _) = Pubkey::find_program_address(
        &[
            b"pool",
            &config0.to_bytes(),
            &t0_pk.to_bytes(),
            &t1_pk.to_bytes(),
        ],
        &cpmm,
    );

    let (pool_addr, config_used) = if rpc.get_account(&pool0).await.is_ok() {
        (pool0, config0)
    } else {
        // Fallback: batch-check config indices 1-4
        let configs: Vec<(Pubkey, Pubkey)> = (1u16..5u16)
            .map(|idx| {
                let (cfg, _) =
                    Pubkey::find_program_address(&[b"amm_config", &idx.to_be_bytes()], &cpmm);
                let (pool, _) = Pubkey::find_program_address(
                    &[
                        b"pool",
                        &cfg.to_bytes(),
                        &t0_pk.to_bytes(),
                        &t1_pk.to_bytes(),
                    ],
                    &cpmm,
                );
                (cfg, pool)
            })
            .collect();

        let pool_pks: Vec<Pubkey> = configs.iter().map(|(_, p)| *p).collect();
        let results = rpc.get_multiple_accounts(&pool_pks).await.ok()?;

        let mut found: Option<(Pubkey, Pubkey)> = None;
        for (i, acct_opt) in results.iter().enumerate() {
            if acct_opt.is_some() {
                found = Some(configs[i]);
                break;
            }
        }
        found?
    };

    let account = rpc.get_account(&pool_addr).await.ok()?;
    let data = &account.data;

    if data.len() < 232 {
        log::debug!(
            "[CPMM] pool data too short: {} bytes (need >=232 for mints at offset 200)",
            data.len()
        );
        return None;
    }

    let read_pubkey = |off: usize| -> Option<String> {
        let bytes: [u8; 32] = data[off..off + 32].try_into().ok()?;
        Some(Pubkey::new_from_array(bytes).to_string())
    };

    let state_mint_0 = read_pubkey(TOKEN0_MINT_OFFSET)?;
    let state_mint_1 = read_pubkey(TOKEN1_MINT_OFFSET)?;

    if (state_mint_0 != *token_0 || state_mint_1 != *token_1)
        && (state_mint_0 != *token_1 || state_mint_1 != *token_0)
    {
        log::debug!(
            "[CPMM] mint mismatch: expected {}/{} got {}/{}",
            &token_0[..8],
            &token_1[..8],
            &state_mint_0[..8],
            &state_mint_1[..8]
        );
        return None;
    }

    let (vault_0_addr, _) = Pubkey::find_program_address(
        &[b"pool_vault", &pool_addr.to_bytes(), &t0_pk.to_bytes()],
        &cpmm,
    );
    let (vault_1_addr, _) = Pubkey::find_program_address(
        &[b"pool_vault", &pool_addr.to_bytes(), &t1_pk.to_bytes()],
        &cpmm,
    );

    let (amt_0, amt_1) = match rpc
        .get_multiple_accounts(&[vault_0_addr, vault_1_addr])
        .await
    {
        Ok(accts) => {
            let a0 = accts
                .first()
                .and_then(|a| a.as_ref())
                .and_then(|a| parse_token_amount(&a.data))
                .unwrap_or(0);
            let a1 = accts
                .get(1)
                .and_then(|a| a.as_ref())
                .and_then(|a| parse_token_amount(&a.data))
                .unwrap_or(0);
            (a0, a1)
        }
        Err(_) => (0, 0),
    };

    let state = PoolStateData {
        pool_pubkey: pool_addr.to_string(),
        config_pubkey: config_used.to_string(),
        token_0_mint: state_mint_0,
        token_1_mint: state_mint_1,
        token_0_vault: vault_0_addr.to_string(),
        token_1_vault: vault_1_addr.to_string(),
        token_0_vault_raw: amt_0,
        token_1_vault_raw: amt_1,
    };
    // Persist for fast restart
    crate::persistence::cpmm_pool_save(
        &state.token_0_mint, &state.token_1_mint,
        &state.pool_pubkey, &state.config_pubkey,
    );
    log::debug!(
        "[CPMM] pool fetched: {} mint0={} mint1={} v0_raw={} v1_raw={}",
        &state.pool_pubkey[..12.min(state.pool_pubkey.len())],
        &state.token_0_mint[..12.min(state.token_0_mint.len())],
        &state.token_1_mint[..12.min(state.token_1_mint.len())],
        state.token_0_vault_raw,
        state.token_1_vault_raw,
    );
    Some(state)
}

#[allow(dead_code)]
async fn read_pool_state_at(rpc: &RpcClient, pool_addr: &str) -> Option<PoolStateData> {
    let pool_pk = Pubkey::from_str(pool_addr).ok()?;
    let account = rpc.get_account(&pool_pk).await.ok()?;
    let data = &account.data;

    if data.len() < 232 {
        log::debug!(
            "[CPMM] pool data too short at {pool_addr}: {} bytes",
            data.len()
        );
        return None;
    }

    let read_pubkey = |off: usize| -> Option<String> {
        let bytes: [u8; 32] = data[off..off + 32].try_into().ok()?;
        Some(Pubkey::new_from_array(bytes).to_string())
    };

    let token_0_mint = read_pubkey(TOKEN0_MINT_OFFSET)?;
    let token_1_mint = read_pubkey(TOKEN1_MINT_OFFSET)?;
    let config_pubkey = read_pubkey(8)?;

    let cpmm = Pubkey::from_str(CPMM_PROGRAM).ok()?;
    let t0_pk = Pubkey::from_str(&token_0_mint).ok()?;
    let t1_pk = Pubkey::from_str(&token_1_mint).ok()?;

    let (vault_0_addr, _) = Pubkey::find_program_address(
        &[b"pool_vault", &pool_pk.to_bytes(), &t0_pk.to_bytes()],
        &cpmm,
    );
    let (vault_1_addr, _) = Pubkey::find_program_address(
        &[b"pool_vault", &pool_pk.to_bytes(), &t1_pk.to_bytes()],
        &cpmm,
    );

    let (amt_0, amt_1) = match rpc
        .get_multiple_accounts(&[vault_0_addr, vault_1_addr])
        .await
    {
        Ok(accts) => {
            let a0 = accts
                .first()
                .and_then(|a| a.as_ref())
                .and_then(|a| parse_token_amount(&a.data))
                .unwrap_or(0);
            let a1 = accts
                .get(1)
                .and_then(|a| a.as_ref())
                .and_then(|a| parse_token_amount(&a.data))
                .unwrap_or(0);
            (a0, a1)
        }
        Err(_) => (0, 0),
    };

    let state = PoolStateData {
        pool_pubkey: pool_addr.to_string(),
        config_pubkey,
        token_0_mint,
        token_1_mint,
        token_0_vault: vault_0_addr.to_string(),
        token_1_vault: vault_1_addr.to_string(),
        token_0_vault_raw: amt_0,
        token_1_vault_raw: amt_1,
    };
    log::debug!(
        "[CPMM] pool by addr: {} mint0={} mint1={} v0_raw={} v1_raw={}",
        &state.pool_pubkey[..12.min(state.pool_pubkey.len())],
        &state.token_0_mint[..12.min(state.token_0_mint.len())],
        &state.token_1_mint[..12.min(state.token_1_mint.len())],
        state.token_0_vault_raw,
        state.token_1_vault_raw,
    );
    Some(state)
}
