//! Pump.fun bonding curve cache fetching (Phase 6+7+9)
//!
//! Read bonding curve virtual reserves and protocol fee recipient address from chain.
//! Phase 9: support PumpSwap AMM pool (pAMMBay6oceH) for graduated tokens.
//!
//! Priority: coins-v2 HTTP API (only reliable path) → old bonding curve PDA fallback (Pattern 2)

mod pda;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::LazyLock;
use tokio::sync::RwLock;

use super::{BondingCurveState, CacheEntry, PumpVenueKind, BONDING_CURVE_CACHE};
use crate::config;
use crate::constants::{PUMPFUN_AMM_PROGRAM, PUMPFUN_BONDING_CURVE_PROGRAM};

use pda::read_pumpswap_pool;

/// Fetch PumpSwap AMM pool reserves fresh from chain, bypassing the cache.
///
/// Used before transaction submission (R2-M01) to ensure pricing is based on
/// current vault balances, not a 10-second-old cache entry.
#[allow(dead_code)]
pub async fn fetch_pumpswap_reserves_fresh(
    rpc: &RpcClient,
    pool: &Pubkey,
    meme_mint: &str,
) -> anyhow::Result<(u64, u64)> {
    let state = read_pumpswap_pool(rpc, pool, &pool.to_string(), meme_mint)
        .await
        .ok_or_else(|| anyhow::anyhow!("failed to read PumpSwap pool {}", pool))?;
    Ok((state.virtual_sol_reserves, state.virtual_token_reserves))
}

/// Static reqwest client — reads proxy from config.toml or HTTPS_PROXY env var
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    let proxy_url = config::load_config_proxy_url().or_else(|| std::env::var("HTTPS_PROXY").ok());
    config::create_http_client(&proxy_url)
});

/// Permanent cache of `mint → PumpSwap pool address`.
/// Pool addresses are immutable after creation, so entries never expire.
static PUMP_SWAP_POOL_ADDR_CACHE: LazyLock<RwLock<HashMap<String, Pubkey>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Look up the PumpSwap pool address for a mint (sync, no RPC fallback).
///
/// Checks the permanent `PUMP_SWAP_POOL_ADDR_CACHE` first, then falls back to
/// `BONDING_CURVE_CACHE` (even if stale — the pool address is static).
pub fn get_pumpswap_pool_address(mint: &str) -> Option<Pubkey> {
    // Fast path: permanent cache
    {
        let cache = PUMP_SWAP_POOL_ADDR_CACHE.try_read().ok()?;
        if let Some(addr) = cache.get(mint) {
            return Some(*addr);
        }
    }
    // Fallback: bonding curve cache (pool address stored as bonding_curve_address)
    {
        let cache = BONDING_CURVE_CACHE.try_read().ok()?;
        if let Some(entry) = cache.get(mint) {
            if let Some(ref state) = entry.data {
                if state.venue_kind == PumpVenueKind::PumpSwapPool {
                    if let Ok(pk) = Pubkey::from_str(&state.bonding_curve_address) {
                        // Backfill permanent cache
                        if let Ok(mut pc) = PUMP_SWAP_POOL_ADDR_CACHE.try_write() {
                            pc.insert(mint.to_string(), pk);
                        }
                        return Some(pk);
                    }
                }
            }
        }
    }
    None
}

/// Resolve PumpSwap pool address for a mint with on-chain fallback.
///
/// 1. Check caches (permanent + BONDING_CURVE_CACHE)
/// 2. If still missing, read the bonding curve account to extract `creator`,
///    then derive the pool PDA via `["pool", 0u16::LE, creator, mint, SOL]`.
/// 3. Cache the result in `PUMP_SWAP_POOL_ADDR_CACHE` for future calls.
pub async fn resolve_pumpswap_pool_address(
    rpc: &RpcClient,
    mint: &str,
) -> Option<Pubkey> {
    // Fast path: caches
    if let Some(addr) = get_pumpswap_pool_address(mint) {
        return Some(addr);
    }

    // Slow path: read bonding curve account, extract creator, derive pool PDA
    let mint_pk = Pubkey::from_str(mint).ok()?;
    let bc_prog = Pubkey::from_str(PUMPFUN_BONDING_CURVE_PROGRAM).ok()?;
    let (bc_pda, _) =
        Pubkey::find_program_address(&[b"bonding-curve", &mint_pk.to_bytes()], &bc_prog);

    let bc_acct = rpc.get_account(&bc_pda).await.ok()?;
    let data = &bc_acct.data;
    // Need at least 81 bytes: discriminator(8) + through creator(49+32=81)
    if data.len() < 81 {
        log::debug!("[PUMP] BC too short for creator mint={}", &mint[..mint.len().min(12)]);
        return None;
    }
    let creator = Pubkey::new_from_array(data[49..81].try_into().unwrap());

    let amm_prog = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).ok()?;
    let sol_mint = Pubkey::from_str(crate::constants::NATIVE_SOL_MINT).ok()?;
    let (pool_addr, _) = Pubkey::find_program_address(
        &[
            b"pool",
            &0u16.to_le_bytes(),
            &creator.to_bytes(),
            &mint_pk.to_bytes(),
            &sol_mint.to_bytes(),
        ],
        &amm_prog,
    );

    // Cache for future calls
    if let Ok(mut cache) = PUMP_SWAP_POOL_ADDR_CACHE.try_write() {
        cache.insert(mint.to_string(), pool_addr);
    }
    log::debug!(
        "[PUMP] derived pool={} from BC creator mint={}",
        &pool_addr.to_string()[..pool_addr.to_string().len().min(12)],
        &mint[..mint.len().min(12)],
    );

    Some(pool_addr)
}

// ============================================================
// Coin metadata from Pump.fun HTTP API
// ============================================================

/// Result from `coins-v2/{mint}` — the official SDK approach for coin state detection
#[derive(Debug, Clone)]
struct CoinV2Meta {
    bonding_curve: String,
    pump_swap_pool: Option<String>,
    complete: bool,
}

/// Query `https://frontend-api-v3.pump.fun/coins-v2/{mint}`.
/// This is the same API used by the official pump.fun swap skill and SDK.
async fn fetch_coin_v2(mint: &str) -> Option<CoinV2Meta> {
    let url = format!("https://frontend-api-v3.pump.fun/coins-v2/{}", mint);
    let resp = HTTP_CLIENT.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let bonding_curve = json.get("bonding_curve")?.as_str()?.to_string();
    let pump_swap_pool = json
        .get("pump_swap_pool")
        .and_then(|v| v.as_str())
        .map(String::from);
    let complete = json
        .get("complete")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some(CoinV2Meta {
        bonding_curve,
        pump_swap_pool,
        complete,
    })
}

// ============================================================
// Account parsing helpers
// ============================================================

/// Read u64 from byte slice at offset (little-endian)
fn read_u64_at(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap_or([0; 8]))
}

/// Read Pubkey from byte slice at offset
fn read_pubkey_at(data: &[u8], off: usize) -> Pubkey {
    Pubkey::new_from_array(data[off..off + 32].try_into().unwrap())
}

// ============================================================
// Path B: Old bonding curve (via API-provided address)
// ============================================================

/// Read bonding curve account at a known address.
/// Layout (after 8-byte discriminator):
///   offset  8: virtualTokenReserves (u64)
///   offset 16: virtualSolReserves (u64)
///   offset 24: realTokenReserves (u64)
///   offset 32: realSolReserves (u64)
///   offset 40: tokenTotalSupply (u64)
///   offset 48: complete (bool)
///   offset 49: creator (Pubkey)
async fn read_bonding_curve_at(
    rpc: &RpcClient,
    bc_pk: &Pubkey,
    bc_addr: &str,
    mint: &str,
) -> Option<BondingCurveState> {
    let acct = rpc.get_account(bc_pk).await.ok()?;
    let data = &acct.data;
    if data.len() < 49 {
        return None;
    }

    let creator = if data.len() >= 81 {
        read_pubkey_at(data, 49).to_string()
    } else {
        String::new()
    };

    Some(BondingCurveState {
        mint: mint.to_string(),
        bonding_curve_address: bc_addr.to_string(),
        virtual_token_reserves: read_u64_at(data, 8),
        virtual_sol_reserves: read_u64_at(data, 16),
        real_token_reserves: read_u64_at(data, 24),
        real_sol_reserves: read_u64_at(data, 32),
        complete: data[48] != 0,
        creator,
        venue_kind: PumpVenueKind::BondingCurve,
    })
}

// ============================================================
// Main entry points
// ============================================================

/// Fetch Pump.fun fee recipient from on-chain global config
pub async fn fetch_pumpfun_fee_recipient(rpc: &RpcClient) -> Option<String> {
    let programs = [PUMPFUN_AMM_PROGRAM, PUMPFUN_BONDING_CURVE_PROGRAM];
    let global_seed = b"global";

    for prog_str in &programs {
        let prog = Pubkey::from_str(prog_str).ok()?;
        let (global_pda, _) = Pubkey::find_program_address(&[global_seed], &prog);
        if let Ok(acct) = rpc.get_account(&global_pda).await {
            let data = &acct.data;
            if data.len() >= 73 {
                let bytes: [u8; 32] = data[41..73].try_into().ok()?;
                let fee_recipient = Pubkey::new_from_array(bytes).to_string();
                log::info!("[PUMPFUN] fee_recipient from chain: {}", fee_recipient);
                return Some(fee_recipient);
            }
        }
    }
    None
}

/// Fetch + cache Pump.fun price data.
///
/// Three paths, tried in order:
///   1. HTTP API (`coins-v2/{mint}`) → PumpSwap pool for graduated tokens
///   2. HTTP API → bonding curve address for pre-graduation tokens
///   3. PDA fallback: bonding curve (old Pump program, deterministic)
///
/// Note: PumpSwap pool PDA is NOT deterministically derivable from mint alone,
/// because Pool.creator is the wallet that called migrate (permissionless, varies per pool).
/// Verified 2026-06-13: BC_PDA ≠ creator, and creator ≠ constant across pools.
pub async fn fetch_bonding_curve(rpc: &RpcClient, mint: &str) -> Option<BondingCurveState> {
    // Check cache with short TTL — meme coin prices stale quickly
    {
        let cache = BONDING_CURVE_CACHE.read().ok()?;
        if let Some(entry) = cache.get(mint) {
            if entry.age_secs() < super::PUMPFUN_RESERVE_TTL_SECS {
                return entry.data.clone();
            }
        }
    }

    let cache_miss = |state: Option<BondingCurveState>| {
        if let Ok(mut cache) = BONDING_CURVE_CACHE.write() {
            cache.insert(mint.to_string(), CacheEntry::new(state.clone()));
        } else {
            let mut cache = BONDING_CURVE_CACHE.write().unwrap_or_else(|e| {
                log::warn!("BondingCurve cache poisoned, recovering");
                e.into_inner()
            });
            cache.insert(mint.to_string(), CacheEntry::new(state.clone()));
        }
        state
    };

    let cache_negative = || {
        if let Ok(mut cache) = BONDING_CURVE_CACHE.write() {
            cache.insert(mint.to_string(), CacheEntry::new_negative(None));
        }
    };

    // ---- Path 1 & 2: HTTP API (official SDK approach) ----
    if let Some(meta) = fetch_coin_v2(mint).await {
        // Cache pool address in permanent map (static, never expires)
        if let Some(ref pool_addr) = meta.pump_swap_pool {
            if let Ok(pool_pk) = Pubkey::from_str(pool_addr) {
                if let Ok(mut cache) = PUMP_SWAP_POOL_ADDR_CACHE.try_write() {
                    cache.insert(mint.to_string(), pool_pk);
                }
            }
        }

        // Path 1: Graduated → use PumpSwap pool address directly
        if meta.complete {
            if let Some(ref pool_addr) = meta.pump_swap_pool {
                if let Ok(pool_pk) = Pubkey::from_str(pool_addr) {
                    if let Some(state) = read_pumpswap_pool(rpc, &pool_pk, pool_addr, mint).await {
                        log::debug!(
                            "[PUMPFUN] PumpSwap via API mint={} sol_res={} tok_res={}",
                            &mint[..mint.len().min(8)],
                            state.virtual_sol_reserves,
                            state.virtual_token_reserves,
                        );
                        return cache_miss(Some(state));
                    }
                }
                // Pool address exists but couldn't read — token may be migrating
                log::debug!(
                    "[PUMPFUN] PumpSwap pool read failed mint={}, falling back",
                    &mint[..mint.len().min(8)],
                );
            }
        }

        // Path 2: Pre-graduation → use bonding curve address from API
        if let Ok(bc_pk) = Pubkey::from_str(&meta.bonding_curve) {
            if let Some(state) = read_bonding_curve_at(rpc, &bc_pk, &meta.bonding_curve, mint).await
            {
                if state.virtual_sol_reserves > 0 && state.virtual_token_reserves > 0 {
                    log::debug!(
                        "[PUMPFUN] BC via API mint={} sol_res={} tok_res={} complete={}",
                        &mint[..mint.len().min(8)],
                        state.virtual_sol_reserves,
                        state.virtual_token_reserves,
                        state.complete,
                    );
                    return cache_miss(Some(state));
                }
                // Bonding curve drained — token may be in migration
                log::debug!(
                    "[PUMPFUN] BC drained via API mint={} complete={}, trying fallback",
                    &mint[..mint.len().min(8)],
                    state.complete,
                );
            }
        }

        // If API said complete=true but we couldn't read the pool, the token
        // might be in a migration window. Don't cache negative — retry next time.
        if meta.complete && meta.pump_swap_pool.is_some() {
            return None; // let next poll retry
        }
    }

    // ---- Path 3: Fallback — PDA derivation for old bonding curve ----
    // Only reached if API is down or returns unusable data
    log::trace!(
        "[PUMPFUN] API unavailable for mint={}, trying PDA fallback",
        &mint[..mint.len().min(8)],
    );

    let mint_pk = Pubkey::from_str(mint).ok()?;
    let bc_prog = Pubkey::from_str(PUMPFUN_BONDING_CURVE_PROGRAM).ok()?;

    let (bc_pda, _) =
        Pubkey::find_program_address(&[b"bonding-curve", &mint_pk.to_bytes()], &bc_prog);

    match rpc.get_account(&bc_pda).await {
        Ok(acct) if acct.data.len() >= 49 => {
            let data = &acct.data;
            let virtual_sol = read_u64_at(data, 16);
            let virtual_tok = read_u64_at(data, 8);

            if virtual_sol > 0 && virtual_tok > 0 {
                let creator = if data.len() >= 81 {
                    read_pubkey_at(data, 49).to_string()
                } else {
                    String::new()
                };
                let state = BondingCurveState {
                    mint: mint.to_string(),
                    bonding_curve_address: bc_pda.to_string(),
                    virtual_token_reserves: virtual_tok,
                    virtual_sol_reserves: virtual_sol,
                    real_token_reserves: read_u64_at(data, 24),
                    real_sol_reserves: read_u64_at(data, 32),
                    complete: data[48] != 0,
                    creator,
                    venue_kind: PumpVenueKind::BondingCurve,
                };
                log::debug!(
                    "[PUMPFUN] BC via PDA fallback mint={} sol_res={} tok_res={}",
                    &mint[..mint.len().min(8)],
                    virtual_sol,
                    virtual_tok,
                );
                return cache_miss(Some(state));
            }

            // Bonding curve drained — token graduated via migrate.
            // PumpSwap pool PDA needs Pool.creator (permissionless, varies per pool).
            // Without the API, graduated token pool addresses cannot be resolved.
            log::debug!(
                "[PUMPFUN] BC drained mint={} graduated={}, no pool PDA available without API",
                &mint[..mint.len().min(8)],
                data[48] != 0,
            );
        }
        Ok(acct) => {
            log::trace!(
                "[PUMPFUN] PDA BC too short mint={} len={}",
                &mint[..mint.len().min(8)],
                acct.data.len(),
            );
        }
        Err(e) => {
            log::trace!(
                "[PUMPFUN] PDA BC not found mint={}: {e}",
                &mint[..mint.len().min(8)],
            );
        }
    }

    cache_negative();
    None
}
