//! Orca Whirlpool pool cache fetching
//!
//! PDA derivation of pool address (with tick_spacing iteration), parse borsh Whirlpool,
//! read vault balances.

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::{
    cache_key, compute_liquidity_sol, parse_token_amount, CacheEntry, DlmmPoolReserves,
    WHIRLPOOL_CACHE,
};
use crate::constants::{WHIRLPOOL_CONFIG, WHIRLPOOL_PROGRAM};

/// Load persisted Whirlpool pool addresses into cache at startup.
pub fn load_whirlpool_pools() {
    let rows = crate::persistence::whirlpool_pools_load_all();
    let mut cache = WHIRLPOOL_CACHE.write().unwrap_or_else(|e| {
        log::warn!("Whirlpool cache poisoned, recovering");
        e.into_inner()
    });
    for (mint_a, mint_b, pool_addr, tick_spacing) in &rows {
        let key = cache_key(mint_a, mint_b);
        if !cache.contains_key(&key) {
            let state = DlmmPoolReserves {
                lb_pair: pool_addr.clone(),
                token_x_mint: mint_a.clone(),
                token_y_mint: mint_b.clone(),
                reserve_x: 0, reserve_y: 0,
                reserve_x_address: String::new(), reserve_y_address: String::new(),
                bin_array_addresses: vec![], bins: vec![],
                bin_step: *tick_spacing as u16, base_factor: 0, active_id: 0,
                bin_array_bitmap_extension: None,
                sqrt_price: 0, tick_current_index: 0, fee_rate: 0,
            };
            cache.insert(key, CacheEntry::new_persisted(Some(state)));
        }
    }
    log::info!("[WHIRLPOOL] loaded {} pools from DB", rows.len());
}

/// Common tick spacings to try when deriving pool PDA, ordered by likelihood for SOL pairs
const TICK_SPACINGS: &[u16] = &[64, 128, 1, 8, 256, 32];

/// Offsets in Whirlpool account data (raw, includes 8-byte anchor discriminator).
/// Verified against mainnet pool Gjf1WWobRjjLW6EBXPXyqdqaGM56ySuPzU6eufs3xzod (2026-06-20)
/// and official Orca 0.35.2 test_whirlpool_data_layout.
///
/// Layout: discriminator(8) | whirlpools_config(32) | bump(1) | tick_spacing(2) |
///   fee_tier_seed(2) | fee_rate(2) | protocol_fee_rate(2) | liquidity(16) |
///   sqrt_price(16) | tick_current_index(4) | protocol_fee_owed_a(8) |
///   protocol_fee_owed_b(8) | token_mint_a(32) | token_vault_a(32) |
///   fee_growth_global_a(16) | token_mint_b(32) | token_vault_b(32) |
///   fee_growth_global_b(16) | reward_last_updated(8) | reward_infos[3](384)
const TOKEN_MINT_A_OFF: usize = 101;
const TOKEN_VAULT_A_OFF: usize = 133;
const TOKEN_MINT_B_OFF: usize = 181;
const TOKEN_VAULT_B_OFF: usize = 213;
const MIN_DATA_LEN: usize = 245; // through token_vault_b + 32

/// sqrt_price (u128 LE) at raw offset 65 (verified against mainnet).
const SQRT_PRICE_OFF: usize = 65;
/// tick_current_index (i32 LE) at raw offset 81 (verified against mainnet).
const TICK_CURRENT_INDEX_OFF: usize = 81;
/// fee_rate (u16 LE) at raw offset 45. Stored as hundredths of a basis point
/// (3000 = 0.30% = 30 bps). Verified against mainnet.
const FEE_RATE_OFF: usize = 45;

/// Sync fetch + write cache, return SOL-equivalent liquidity (DLMM)
#[allow(dead_code)]
pub async fn fetch_whirlpool_now(rpc: &RpcClient, t0: &str, t1: &str) -> Option<f64> {
    let key = cache_key(t0, t1);
    if let Some(entry) = WHIRLPOOL_CACHE.read().ok()?.get(&key) {
        if !entry.is_stale() {
            return entry.data.as_ref().and_then(|r| {
                compute_liquidity_sol(&super::PoolReserves {
                    token_0: r.token_x_mint.clone(),
                    token_1: r.token_y_mint.clone(),
                    token_0_vault_raw: r.reserve_x,
                    token_1_vault_raw: r.reserve_y,
                })
            });
        }
    }

    let state = fetch_whirlpool_pool(rpc, t0, t1).await;
    let liq = state.as_ref().and_then(|r| {
        compute_liquidity_sol(&super::PoolReserves {
            token_0: r.token_x_mint.clone(),
            token_1: r.token_y_mint.clone(),
            token_0_vault_raw: r.reserve_x,
            token_1_vault_raw: r.reserve_y,
        })
    });
    let mut cache = WHIRLPOOL_CACHE.write().unwrap_or_else(|e| {
        log::warn!("Whirlpool cache poisoned, recovering");
        e.into_inner()
    });
    cache.insert(key, super::CacheEntry::new(state));
    liq
}

/// Fetch Whirlpool reserves for a pair, caching the result
pub async fn fetch_whirlpool_by_mints(
    rpc: &RpcClient,
    mint_a: &str,
    mint_b: &str,
) -> Option<DlmmPoolReserves> {
    let key = cache_key(mint_a, mint_b);
    if let Some(entry) = WHIRLPOOL_CACHE.read().ok()?.get(&key) {
        if !entry.is_stale() {
            return entry.data.clone();
        }
    }

    let state = fetch_whirlpool_pool(rpc, mint_a, mint_b).await;
    let mut cache = WHIRLPOOL_CACHE.write().unwrap_or_else(|e| {
        log::warn!("Whirlpool cache poisoned, recovering");
        e.into_inner()
    });
    cache.insert(key, super::CacheEntry::new(state.clone()));
    state
}

// ============================================================
// Internal implementation
// ============================================================

async fn fetch_whirlpool_pool(rpc: &RpcClient, t0: &str, t1: &str) -> Option<DlmmPoolReserves> {
    let whirlpool_prog = Pubkey::from_str(WHIRLPOOL_PROGRAM).ok()?;
    let config = Pubkey::from_str(WHIRLPOOL_CONFIG).ok()?;

    // Sort mints for PDA derivation (Orca requires sorted mints)
    let (mint_a, mint_b) = if t0 < t1 { (t0, t1) } else { (t1, t0) };
    let mint_a_pk = Pubkey::from_str(mint_a).ok()?;
    let mint_b_pk = Pubkey::from_str(mint_b).ok()?;

    // Try each tick spacing to find the pool PDA
    let mut candidates: Vec<(Pubkey, u16)> = Vec::with_capacity(TICK_SPACINGS.len());
    for &ts in TICK_SPACINGS {
        let (pool_pda, _) = Pubkey::find_program_address(
            &[
                b"whirlpool",
                &config.to_bytes(),
                &mint_a_pk.to_bytes(),
                &mint_b_pk.to_bytes(),
                &ts.to_le_bytes(),
            ],
            &whirlpool_prog,
        );
        candidates.push((pool_pda, ts));
    }

    let pool_pks: Vec<Pubkey> = candidates.iter().map(|(p, _)| *p).collect();
    let results = rpc.get_multiple_accounts(&pool_pks).await.ok()?;

    let mut found_idx: Option<usize> = None;
    for (i, acct_opt) in results.iter().enumerate() {
        if let Some(acct) = acct_opt {
            if acct.owner == whirlpool_prog && acct.data.len() >= MIN_DATA_LEN {
                found_idx = Some(i);
                break;
            }
        }
    }

    let (pool_addr, tick_spacing) = candidates[found_idx?];
    let pool_data = &results[found_idx?].as_ref()?.data;

    let read_pubkey = |off: usize| -> Option<String> {
        let bytes: [u8; 32] = pool_data[off..off + 32].try_into().ok()?;
        Some(Pubkey::new_from_array(bytes).to_string())
    };
    let read_u128 = |off: usize| -> Option<u128> {
        let bytes: [u8; 16] = pool_data[off..off + 16].try_into().ok()?;
        Some(u128::from_le_bytes(bytes))
    };
    let read_i32 = |off: usize| -> Option<i32> {
        let bytes: [u8; 4] = pool_data[off..off + 4].try_into().ok()?;
        Some(i32::from_le_bytes(bytes))
    };

    let token_mint_a_str = read_pubkey(TOKEN_MINT_A_OFF)?;
    let token_mint_b_str = read_pubkey(TOKEN_MINT_B_OFF)?;
    let vault_a_str = read_pubkey(TOKEN_VAULT_A_OFF)?;
    let vault_b_str = read_pubkey(TOKEN_VAULT_B_OFF)?;

    // Read sqrt_price and tick_current_index for pricing and tick array derivation
    let sqrt_price = read_u128(SQRT_PRICE_OFF).unwrap_or(0);
    let tick_current_index = read_i32(TICK_CURRENT_INDEX_OFF).unwrap_or(0);
    let fee_rate = {
        let bytes: [u8; 2] = pool_data[FEE_RATE_OFF..FEE_RATE_OFF + 2].try_into().ok()?;
        u16::from_le_bytes(bytes)
    };

    // Read vault token accounts to get reserve amounts
    let vault_a_pk = Pubkey::from_str(&vault_a_str).ok()?;
    let vault_b_pk = Pubkey::from_str(&vault_b_str).ok()?;

    let (amt_a, amt_b) = match rpc.get_multiple_accounts(&[vault_a_pk, vault_b_pk]).await {
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

    let (reserve_x, reserve_y) = if token_mint_a_str == *mint_a {
        (amt_a, amt_b)
    } else {
        (amt_b, amt_a)
    };

    log::debug!(
        "[WHIRLPOOL] pool={} ts={} mints={}/{} reserves={}/{} sqrt_price={} tick={}",
        &pool_addr.to_string()[..12.min(pool_addr.to_string().len())],
        tick_spacing,
        &token_mint_a_str[..12.min(token_mint_a_str.len())],
        &token_mint_b_str[..12.min(token_mint_b_str.len())],
        amt_a,
        amt_b,
        sqrt_price,
        tick_current_index,
    );

    // Persist for fast restart
    crate::persistence::whirlpool_pool_save(
        &token_mint_a_str, &token_mint_b_str,
        &pool_addr.to_string(), tick_spacing,
    );

    Some(DlmmPoolReserves {
        lb_pair: pool_addr.to_string(),
        token_x_mint: token_mint_a_str,
        token_y_mint: token_mint_b_str,
        reserve_x,
        reserve_y,
        reserve_x_address: vault_a_str,
        reserve_y_address: vault_b_str,
        bin_array_addresses: vec![],
        bins: vec![],
        bin_step: tick_spacing,
        base_factor: 0,
        active_id: 0,
        bin_array_bitmap_extension: None,
        sqrt_price,
        tick_current_index,
        fee_rate,
    })
}

/// Read reserves from known pool address (pools discovered by listener, no PDA derivation needed)
pub async fn fetch_whirlpool_by_address(
    rpc: &RpcClient,
    pool_addr: &str,
) -> Option<DlmmPoolReserves> {
    let pool_pk = Pubkey::from_str(pool_addr).ok()?;
    let account = rpc.get_account(&pool_pk).await.ok()?;
    let data = &account.data;

    if data.len() < MIN_DATA_LEN {
        return None;
    }

    let read_pubkey = |off: usize| -> Option<String> {
        let bytes: [u8; 32] = data[off..off + 32].try_into().ok()?;
        Some(Pubkey::new_from_array(bytes).to_string())
    };

    let token_mint_a = read_pubkey(TOKEN_MINT_A_OFF)?;
    let token_mint_b = read_pubkey(TOKEN_MINT_B_OFF)?;
    let vault_a = read_pubkey(TOKEN_VAULT_A_OFF)?;
    let vault_b = read_pubkey(TOKEN_VAULT_B_OFF)?;

    let sqrt_price = {
        let bytes: [u8; 16] = data[SQRT_PRICE_OFF..SQRT_PRICE_OFF + 16].try_into().ok()?;
        u128::from_le_bytes(bytes)
    };
    let tick_current_index = {
        let bytes: [u8; 4] =
            data[TICK_CURRENT_INDEX_OFF..TICK_CURRENT_INDEX_OFF + 4].try_into().ok()?;
        i32::from_le_bytes(bytes)
    };
    let fee_rate = {
        let bytes: [u8; 2] = data[FEE_RATE_OFF..FEE_RATE_OFF + 2].try_into().ok()?;
        u16::from_le_bytes(bytes)
    };

    let vault_a_pk = Pubkey::from_str(&vault_a).ok()?;
    let vault_b_pk = Pubkey::from_str(&vault_b).ok()?;

    let (amt_a, amt_b) = match rpc.get_multiple_accounts(&[vault_a_pk, vault_b_pk]).await {
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

    Some(DlmmPoolReserves {
        lb_pair: pool_addr.to_string(),
        token_x_mint: token_mint_a,
        token_y_mint: token_mint_b,
        reserve_x: amt_a,
        reserve_y: amt_b,
        reserve_x_address: vault_a,
        reserve_y_address: vault_b,
        bin_array_addresses: vec![],
        bins: vec![],
        bin_step: 0,
        base_factor: 0,
        active_id: 0,
        bin_array_bitmap_extension: None,
        sqrt_price,
        tick_current_index,
        fee_rate,
    })
}
