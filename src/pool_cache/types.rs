//! Shared types and static caches for the pool_cache module.
//!
//! Contains all data structures (PoolReserves, DlmmBin, BondingCurveState, etc.),
//! static LazyLock caches, TTL constants, and utility functions (cache_key,
//! parse_token_amount, next_utc_midnight).

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::Instant;

use crate::constants::{
    DEFAULT_DECIMALS, NATIVE_SOL_MINT, SOL_DECIMALS, USDC_DECIMALS, USDC_MINT, USDT_MINT,
};

/// TTL for cache entries: entries older than this are treated as stale
pub(crate) const CACHE_TTL_SECS: u64 = 60;
/// TTL for negative cache entries (pool not found).
/// 120s avoids wasting RPC credits on mints that don't have CPMM/Whirlpool pools,
/// while still re-checking periodically in case new pools are created.
pub(crate) const NEGATIVE_CACHE_TTL_SECS: u64 = 120;
/// TTL for DLMM reserve cache: must be short to avoid trading on stale meme prices.
/// 10s balances freshness against RPC credit cost — meme price dislocations rarely
/// disappear faster than this, and the dedup window saves credits during bursts.
pub(crate) const DLMM_RESERVE_TTL_SECS: u64 = 10;
/// TTL for PumpSwap bonding curve cache: also kept short to avoid stale price data.
pub(crate) const PUMPFUN_RESERVE_TTL_SECS: u64 = 10;

/// Unix timestamp (seconds) for next UTC midnight. Used as skip-until for zero-reserve pools.
pub(crate) fn next_utc_midnight() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let secs_in_day: i64 = 86400;
    let days_since_epoch = now / secs_in_day;
    (days_since_epoch + 1) * secs_in_day
}

/// Wrapper around cached data with a last-update timestamp
#[derive(Debug, Clone)]
pub(crate) struct CacheEntry<T> {
    pub(crate) data: T,
    updated_at: Instant,
    negative: bool,
}

impl<T> CacheEntry<T> {
    pub(crate) fn new(data: T) -> Self {
        Self {
            data,
            updated_at: Instant::now(),
            negative: false,
        }
    }

    pub(crate) fn new_negative(data: T) -> Self {
        Self {
            data,
            updated_at: Instant::now(),
            negative: true,
        }
    }

    /// Create a stale-on-arrival entry for DB-loaded data.
    /// Forces the first query to trigger a fresh RPC fetch for actual reserves.
    pub(crate) fn new_persisted(data: T) -> Self {
        Self {
            data,
            updated_at: Instant::now() - std::time::Duration::from_secs(CACHE_TTL_SECS + 1),
            negative: false,
        }
    }

    pub(crate) fn is_stale(&self) -> bool {
        let ttl = if self.negative {
            NEGATIVE_CACHE_TTL_SECS
        } else {
            CACHE_TTL_SECS
        };
        self.updated_at.elapsed().as_secs() > ttl
    }

    /// Seconds since this entry was last updated
    pub(crate) fn age_secs(&self) -> u64 {
        self.updated_at.elapsed().as_secs()
    }
}

/// Compact cache (Phase 3 liquidity calculation)
#[derive(Debug, Clone)]
pub struct PoolReserves {
    pub token_0: String,
    pub token_1: String,
    pub token_0_vault_raw: u64,
    pub token_1_vault_raw: u64,
}

/// Full pool data (for Phase 4 simulator, CPMM only)
#[derive(Debug, Clone)]
pub struct PoolStateData {
    pub pool_pubkey: String,
    #[allow(dead_code)]
    pub config_pubkey: String,
    pub token_0_mint: String,
    pub token_1_mint: String,
    #[allow(dead_code)]
    pub token_0_vault: String,
    #[allow(dead_code)]
    pub token_1_vault: String,
    pub token_0_vault_raw: u64,
    pub token_1_vault_raw: u64,
}

/// AMMv4 full pool info (for Phase 4 simulator)
#[derive(Debug, Clone)]
#[allow(dead_code)] // market_program for future OpenBook integration
pub struct AmmV4PoolInfo {
    pub pool_address: String,
    pub coin_mint: String,
    pub pc_mint: String,
    pub coin_vault: String,
    pub pc_vault: String,
    pub open_orders: String,
    pub target_orders: String,
    pub market: String,
    pub market_program: String,
}

impl PoolReserves {
    pub(crate) fn from_state(s: &PoolStateData) -> Self {
        PoolReserves {
            token_0: s.token_0_mint.clone(),
            token_1: s.token_1_mint.clone(),
            token_0_vault_raw: s.token_0_vault_raw,
            token_1_vault_raw: s.token_1_vault_raw,
        }
    }
}

/// Pump.fun venue type — distinguishes pre-graduation bonding curves from graduated PumpSwap AMM pools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpVenueKind {
    /// Pre-graduation bonding curve (old Pump program). Not executable for cross-pool arbitrage.
    BondingCurve,
    /// Graduated PumpSwap AMM pool. The target venue for cross-pool arbitrage, but not yet
    /// supported in the executor (A-04 PumpSwap swap builder pending).
    PumpSwapPool,
}

/// Pump.fun bonding curve / PumpSwap pool state
#[derive(Debug, Clone)]
#[allow(dead_code)] // real_sol/token_reserves for future pool health checks
pub struct BondingCurveState {
    pub mint: String,
    pub bonding_curve_address: String,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub complete: bool,
    /// Pool creator (bonding curve account offset 49-80), used for PumpSwap PDA derivation
    pub creator: String,
    /// Which Pump.fun venue this state represents
    pub venue_kind: PumpVenueKind,
}

/// Meteora DLMM single bin data (Phase 9 segmented swap estimation)
#[derive(Debug, Clone)]
pub struct DlmmBin {
    pub bin_id: i32,
    /// Token X amount (v1 + v2). For v2, prefer reserve_x for swap calculations.
    pub amount_x: u64,
    /// Token Y amount (v1 + v2). For v2, prefer reserve_y for swap calculations.
    pub amount_y: u64,
    /// Token X reserve (v2 only, 0 for v1). Used by the DLMM swap program.
    pub reserve_x: u64,
    /// Token Y reserve (v2 only, 0 for v1). Used by the DLMM swap program.
    pub reserve_y: u64,
}

/// Meteora DLMM / Orca Whirlpool pool reserves (Phase 6 cross-pool arbitrage)
#[derive(Debug, Clone)]
#[allow(dead_code)] // token_y_mint for future reverse-pair lookups
pub struct DlmmPoolReserves {
    pub lb_pair: String,
    pub token_x_mint: String,
    pub token_y_mint: String,
    pub reserve_x: u64,
    pub reserve_y: u64,
    pub reserve_x_address: String,
    pub reserve_y_address: String,
    pub bin_array_addresses: Vec<String>,
    pub bins: Vec<DlmmBin>,
    pub bin_step: u16,
    pub base_factor: u16,
    pub active_id: i32,
    pub bin_array_bitmap_extension: Option<String>,
    // ── Whirlpool-specific fields (0/default for DLMM) ──
    /// Q64.64 sqrt_price from whirlpool account (0 for DLMM).
    pub sqrt_price: u128,
    /// Current tick index from whirlpool account (0 for DLMM).
    pub tick_current_index: i32,
    /// Fee rate in hundredths of a basis point (3000 = 0.30%). 0 for DLMM.
    pub fee_rate: u16,
}

/// DLMM pool metadata permanent cache — fields that do not change after creation.
/// Key = "mint_x:mint_y" (sorted), persisted to SQLite dlmm_metadata table.
/// Used to skip the discovery step and directly pull latest reserves using known lb_pair address.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DlmmPoolMetadata {
    pub lb_pair: String,
    pub token_x_mint: String,
    pub token_y_mint: String,
    pub bin_step: u16,
    pub base_factor: u16,
    pub bin_array_bitmap_extension: Option<String>,
}

// ============================================================
// Static caches
// ============================================================

/// CPMM cache: Key = "mint0:mint1" (sorted)
/// TTL: 30s, after timeout get_pool_state returns None triggering re-fetch
pub(crate) static CPMM_CACHE: LazyLock<RwLock<HashMap<String, CacheEntry<Option<PoolStateData>>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// CPMM discovered pool address cache (non-PDA pools or non-default config, extracted from transaction logs)
/// Key = "mint0:mint1" (sorted) → pool_address
/// TTL: 300s, pool addresses don't change but periodically refresh to avoid staleness
pub(crate) static DISCOVERED_CPMM_POOLS: LazyLock<RwLock<HashMap<String, CacheEntry<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// AMMv4 cache: Key = "mint0:mint1" (sorted) → Some(SOL liquidity)
/// TTL: 30s, after timeout returns None triggering re-fetch
pub(crate) static AMMV4_CACHE: LazyLock<RwLock<HashMap<String, CacheEntry<Option<f64>>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// AMMv4 pool info cache (for Phase 4 simulator)
/// TTL: 30s, refreshed synchronously with AMMV4_CACHE
pub(crate) static AMMV4_POOL_CACHE: LazyLock<
    RwLock<HashMap<String, CacheEntry<Option<AmmV4PoolInfo>>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// BondingCurve cache: Key = mint
/// TTL: 30s, bonding curve state changes frequently and needs periodic refresh
pub(crate) static BONDING_CURVE_CACHE: LazyLock<
    RwLock<HashMap<String, CacheEntry<Option<BondingCurveState>>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// DLMM cache: Key = "mint_x:mint_y" (sorted)
/// TTL: 3s, after timeout use permanent metadata cache for fast fresh fetch
pub(crate) static DLMM_CACHE: LazyLock<
    RwLock<HashMap<String, CacheEntry<Option<DlmmPoolReserves>>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Key = "mint_x:mint_y" (sorted). Value = all known lb_pairs for this mint pair.
/// Multiple pools can exist for the same token pair (e.g. JLP/SOL has 69 pools with different bin_steps).
pub(crate) static DLMM_POOL_METADATA: LazyLock<RwLock<HashMap<String, Vec<DlmmPoolMetadata>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// lb_pair addresses whose SOL reserves were below `min_reserve_lamports` on last fetch.
/// Value = unix timestamp (seconds) marking the end of skip window (next UTC midnight).
/// These pools are skipped in reserve fetching until the window expires, saving RPC credits.
pub(crate) static DLMM_ZERO_SKIP: LazyLock<RwLock<HashMap<String, i64>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Orca Whirlpool cache: Key = "mint_a:mint_b" (sorted)
/// TTL: 30s, needs re-fetch after timeout. Reuses DlmmPoolReserves (lb_pair field holds pool addr).
pub(crate) static WHIRLPOOL_CACHE: LazyLock<
    RwLock<HashMap<String, CacheEntry<Option<DlmmPoolReserves>>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Orca Whirlpool discovered pool address cache (extracted from WebSocket swap events)
/// Key = "mint_a:mint_b" (sorted) → pool_address
/// TTL: 300s
pub(crate) static DISCOVERED_WHIRLPOOL_POOLS: LazyLock<
    RwLock<HashMap<String, CacheEntry<String>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

// ============================================================
// Utility functions
// ============================================================

pub(crate) fn cache_key(t0: &str, t1: &str) -> String {
    let (a, b) = if t0 < t1 { (t0, t1) } else { (t1, t0) };
    format!("{a}:{b}")
}

/// SPL Token Account parsing: read u64 amount from bytes 64..72
pub(crate) fn parse_token_amount(data: &[u8]) -> Option<u64> {
    if data.len() < 72 {
        return None;
    }
    Some(u64::from_le_bytes(data[64..72].try_into().ok()?))
}

pub(crate) fn guess_decimals(mint: &str) -> u8 {
    if mint == NATIVE_SOL_MINT {
        return SOL_DECIMALS;
    }
    if mint == USDC_MINT || mint == USDT_MINT {
        return USDC_DECIMALS;
    }
    DEFAULT_DECIMALS
}

pub(crate) fn is_stablecoin(mint: &str) -> bool {
    mint == USDC_MINT || mint == USDT_MINT
}
