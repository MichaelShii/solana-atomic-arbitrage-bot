//! Pool liquidity cache (Phase 3+6)
//!
//! Read actual balances of multi-venue pools from chain for cross-pool arbitrage spread calculation.
//!
//! - CPMM: PDA derivation of pool address, parse borsh PoolState
//! - AMMv4: extract vault addresses from transaction instructions, read token account balances
//! - Pump.fun BondingCurve: PDA derivation of bonding curve address, read virtual reserves
//! - Meteora DLMM: read lb_pair and bin array to get active bin reserves
// NOTE: Some AMMv4/CPMM cache functions reserved for future venue expansion.

mod types;
pub(crate) use types::*;

pub mod ammv4;
pub mod bonding_curve;
pub mod cpmm;
pub mod dlmm;
pub mod whirlpool;

pub use bonding_curve::{
    fetch_bonding_curve, fetch_pumpfun_fee_recipient,
    resolve_pumpswap_pool_address,
};
pub use cpmm::{fetch_cpmm_by_address, fetch_cpmm_now, load_cpmm_pools};
pub use dlmm::{cache_dlmm_lb_pair, fetch_bins_fresh, fetch_dlmm_by_mints, load_lb_pair_cache};
pub use whirlpool::{fetch_whirlpool_by_address, fetch_whirlpool_by_mints, load_whirlpool_pools};

/// Return all known DLMM pool metadata entries (for TP cache warmup).
pub fn all_dlmm_metadata() -> Vec<crate::pool_cache::DlmmPoolMetadata> {
    DLMM_POOL_METADATA
        .read()
        .map(|m| m.values().flatten().cloned().collect())
        .unwrap_or_default()
}

use crate::constants::NATIVE_SOL_MINT;

// ============================================================
// Public API
// ============================================================

/// Query CPMM cache to return full PoolStateData (for Phase 4 simulator)
/// Returns None if cache miss or exceeds 30s TTL
pub fn get_pool_state(t0: &str, t1: &str) -> Option<PoolStateData> {
    let key = cache_key(t0, t1);
    let cache = CPMM_CACHE.read().ok()?;
    let entry = cache.get(&key)?;
    if entry.is_stale() {
        return None;
    }
    entry.data.clone()
}

/// Reverse lookup: find CPMM pool by single mint (any trading pair)
/// Full cache scan, O(n). Used as fallback for non-SOL-denominated pair price queries.
pub fn get_pool_state_by_mint(mint: &str) -> Option<PoolStateData> {
    let cache = CPMM_CACHE.read().ok()?;
    for (key, entry) in cache.iter() {
        if entry.is_stale() {
            continue;
        }
        let state = entry.data.as_ref()?;
        // key = "mint_a:mint_b" (sorted), check if mint is either side
        if key.contains(mint) {
            return Some(state.clone());
        }
    }
    None
}

/// Query AMMv4 pool info (for Phase 4 simulator)
/// Returns None if cache miss or exceeds 30s TTL
pub fn get_ammv4_pool_info(mint_a: &str, mint_b: &str) -> Option<AmmV4PoolInfo> {
    let key = cache_key(mint_a, mint_b);
    let cache = AMMV4_POOL_CACHE.read().ok()?;
    let entry = cache.get(&key)?;
    if entry.is_stale() {
        return None;
    }
    entry.data.clone()
}

/// Query DLMM pool reserves
/// Returns None if cache miss or exceeds 30s TTL
pub fn get_dlmm_reserves(token_x: &str, token_y: &str) -> Option<DlmmPoolReserves> {
    let key = cache_key(token_x, token_y);
    let cache = DLMM_CACHE.read().ok()?;
    let entry = cache.get(&key)?;
    if entry.is_stale() {
        return None;
    }
    let hit = entry.data.is_some();
    record_cache_access(hit);
    entry.data.clone()
}

fn record_cache_access(hit: bool) {
    if hit {
        tracing::trace!("cache hit");
    } else {
        tracing::trace!("cache miss");
    }
    let _ = hit; // no-op after removing timing module
}

/// Query Whirlpool pool reserves
pub fn get_whirlpool_reserves(token_x: &str, token_y: &str) -> Option<DlmmPoolReserves> {
    let key = cache_key(token_x, token_y);
    let cache = WHIRLPOOL_CACHE.read().ok()?;
    let entry = cache.get(&key)?;
    if entry.is_stale() {
        return None;
    }
    entry.data.clone()
}

/// Cache Whirlpool pool address discovered from transaction logs
pub fn cache_discovered_whirlpool_pool(mint_a: &str, mint_b: &str, pool_addr: &str) {
    let key = cache_key(mint_a, mint_b);
    let mut cache = DISCOVERED_WHIRLPOOL_POOLS.write().unwrap_or_else(|e| {
        log::warn!("Whirlpool discovered pool cache poisoned, recovering");
        e.into_inner()
    });
    cache.insert(key, CacheEntry::new(pool_addr.to_string()));
}

/// Query discovered Whirlpool pool address
pub fn get_discovered_whirlpool_pool(mint_a: &str, mint_b: &str) -> Option<String> {
    let key = cache_key(mint_a, mint_b);
    let cache = DISCOVERED_WHIRLPOOL_POOLS.read().ok()?;
    let entry = cache.get(&key)?;
    if entry.is_stale() {
        return None;
    }
    Some(entry.data.clone())
}

/// Cache CPMM pool addresses discovered from transaction logs (non-PDA pools or non-default config)
pub fn cache_discovered_cpmm_pool(mint_a: &str, mint_b: &str, pool_addr: &str) {
    let key = cache_key(mint_a, mint_b);
    let mut cache = DISCOVERED_CPMM_POOLS.write().unwrap_or_else(|e| {
        log::warn!("CPMM discovered pool cache poisoned, recovering");
        e.into_inner()
    });
    cache.insert(key, CacheEntry::new(pool_addr.to_string()));
}

/// Query discovered CPMM pool addresses (non-PDA path)
pub fn get_discovered_cpmm_pool(mint_a: &str, mint_b: &str) -> Option<String> {
    let key = cache_key(mint_a, mint_b);
    let cache = DISCOVERED_CPMM_POOLS.read().ok()?;
    let entry = cache.get(&key)?;
    if entry.is_stale() {
        return None;
    }
    Some(entry.data.clone())
}

#[allow(dead_code)] // used when CPMM→DLMM support is added
/// Get SOL-equivalent reserves for a single vault (Phase 3 precise profit calculation)
/// Returns (reserve_a_sol, reserve_b_sol), token order consistent with cache
pub fn get_reserves_sol(t0: &str, t1: &str) -> Option<(f64, f64)> {
    let key = cache_key(t0, t1);
    let cache = CPMM_CACHE.read().ok()?;
    let entry = cache.get(&key)?;
    if entry.is_stale() {
        return None;
    }
    let state = entry.data.as_ref()?;
    let r = PoolReserves::from_state(state);
    let dec0 = guess_decimals(&r.token_0);
    let dec1 = guess_decimals(&r.token_1);
    let ui0 = r.token_0_vault_raw as f64 / 10f64.powi(dec0 as i32);
    let ui1 = r.token_1_vault_raw as f64 / 10f64.powi(dec1 as i32);

    let v0 = vault_sol_value(&r.token_0, ui0);
    let v1 = vault_sol_value(&r.token_1, ui1);

    match (v0, v1) {
        (Some(a), Some(b)) => Some((a, b)),
        (Some(a), None) => Some((a, a)),
        (None, Some(b)) => Some((b, b)),
        (None, None) => None,
    }
}

#[allow(dead_code)] // used when multi-pool SOL valuation is needed
/// SOL-equivalent value of a single vault
fn vault_sol_value(mint: &str, ui_amount: f64) -> Option<f64> {
    if mint == NATIVE_SOL_MINT {
        return Some(ui_amount);
    }
    let sol_price = crate::price::sol_price();
    if sol_price > 0.0 && is_stablecoin(mint) {
        return Some(ui_amount / sol_price);
    }
    None
}

pub(crate) fn compute_liquidity_sol(r: &PoolReserves) -> Option<f64> {
    let dec0 = guess_decimals(&r.token_0);
    let dec1 = guess_decimals(&r.token_1);
    let ui0 = r.token_0_vault_raw as f64 / 10f64.powi(dec0 as i32);
    let ui1 = r.token_1_vault_raw as f64 / 10f64.powi(dec1 as i32);

    if r.token_0 == NATIVE_SOL_MINT {
        return Some(ui0 * 2.0);
    }
    if r.token_1 == NATIVE_SOL_MINT {
        return Some(ui1 * 2.0);
    }

    let sol_price = crate::price::sol_price();
    if sol_price > 0.0 {
        if is_stablecoin(&r.token_0) {
            return Some(ui0 * 2.0 / sol_price);
        }
        if is_stablecoin(&r.token_1) {
            return Some(ui1 * 2.0 / sol_price);
        }
    }

    None
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use crate::constants::CPMM_PROGRAM;
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;

    #[test]
    fn verify_cpmm_pdas() {
        let cpmm = Pubkey::from_str(CPMM_PROGRAM).unwrap();
        let cfg = Pubkey::from_str("D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2").unwrap();
        let cfgb = cfg.to_bytes();

        // Verify config PDA using u16 = D4FPEru...
        let config_pda =
            Pubkey::find_program_address(&[b"amm_config", &0u16.to_le_bytes()], &cpmm).0;
        assert_eq!(config_pda, cfg, "Config PDA mismatch");

        // Pool PDA derivation for SOL-USDC
        let sol = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
        let usdc = Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
        let (st0, st1) = if sol.to_string() < usdc.to_string() {
            (sol, usdc)
        } else {
            (usdc, sol)
        };
        let (pool_addr, _) = Pubkey::find_program_address(
            &[b"pool", &cfgb, &st0.to_bytes(), &st1.to_bytes()],
            &cpmm,
        );

        // Derive vault PDAs
        let (vault0, _) = Pubkey::find_program_address(
            &[b"pool_vault", &pool_addr.to_bytes(), &st0.to_bytes()],
            &cpmm,
        );
        let (vault1, _) = Pubkey::find_program_address(
            &[b"pool_vault", &pool_addr.to_bytes(), &st1.to_bytes()],
            &cpmm,
        );

        println!("Config PDA: {}", config_pda);
        println!("Pool PDA:  {}", pool_addr);
        println!("Vault 0:   {}", vault0);
        println!("Vault 1:   {}", vault1);

        // Verify pool_vault PDA derivation
        let (va0, _) = Pubkey::find_program_address(
            &[b"pool_vault", &pool_addr.to_bytes(), &st0.to_bytes()],
            &cpmm,
        );
        println!("Vault 0 check: {}", va0);
        assert_eq!(vault0, va0);
    }
}
