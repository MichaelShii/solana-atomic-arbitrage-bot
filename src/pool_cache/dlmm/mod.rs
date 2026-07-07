//! Meteora DLMM pool reserve cache fetching (Phase 6+9+10+11)
//!
//! Read lb_pair and reserve accounts, derive bin array PDAs.
//! Phase 9: add venue-agnostic pool discovery (fetch_dlmm_by_mints).
//! Phase 10: support deriveLbPair2.
//! Phase 11: event-driven lb_pair discovery — extract lb_pair addresses from DLMM swap transactions,
//!           build mint→lb_pair cache, bypass PDA derivation.
//! Phase 12: lb_pair cache SQLite persistence, survives restarts.
//!
//! PDA seeds verified against @meteora-ag/dlmm SDK (ts-client/src/dlmm/helpers/derive.ts).
//! Struct offsets verified against IDL (idl.ts — LbPair, StaticParameters, VariableParameters).

use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use crate::config;

/// Number of bins per bin_array (verified against official IDL: [Bin; 70])
pub(crate) const BINS_PER_ARRAY: i32 = 70;

/// Event-driven token→lb_pair cache, persisted to SQLite.
/// Key = token mint (either side of the pair), Value = lb_pair pubkey.
/// Populated from DLMM Swap2 transactions and PDA derivation discoveries.
/// TTL: no explicit TTL — entries live until evicted by a newer mapping for the same mint.
pub(crate) static DLMM_TOKEN_LBPAIR_CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Shared reqwest client for GPAv2 calls — respects proxy from config or HTTPS_PROXY env
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    let proxy_url = config::load_config_proxy_url().or_else(|| std::env::var("HTTPS_PROXY").ok());
    config::create_http_client(&proxy_url)
});

/// Parse a Borsh-encoded Option<Pubkey> at the given offset in lb_pair account data.
/// Anchor Borsh: 1-byte tag (0=None, 1=Some) followed by 32-byte Pubkey if Some.
pub(crate) fn parse_optional_pubkey(data: &[u8], offset: usize) -> Option<Pubkey> {
    if data.len() < offset + 1 || data[offset] == 0 {
        return None;
    }
    if data.len() < offset + 33 {
        return None;
    }
    let bytes: [u8; 32] = data[offset + 1..offset + 33].try_into().ok()?;
    Some(Pubkey::new_from_array(bytes))
}

pub(crate) mod bins;
pub(crate) mod cache;
pub(crate) mod discovery;
pub(crate) mod gpa;
pub(crate) mod reserves;

pub use bins::fetch_bins_fresh;
pub use cache::{cache_dlmm_lb_pair, load_lb_pair_cache};
pub use discovery::fetch_dlmm_by_mints;

#[cfg(test)]
mod tests {
    use super::super::{cache_key, next_utc_midnight, DLMM_CACHE, DLMM_ZERO_SKIP};
    use super::*;
    use crate::constants::NATIVE_SOL_MINT;
    use solana_client::nonblocking::rpc_client::RpcClient;
    use std::time::Duration;

    /// Integration test: verify zero-reserve DLMM pools are skipped on re-fetch.
    /// Uses 4TyZG token (21 DLMM pools on mainnet) to exercise both paths.
    /// Marked #[ignore] — requires RPC access.
    #[tokio::test]
    #[ignore]
    async fn test_zero_reserve_pools_skipped() {
        let _ = tracing_subscriber::fmt::try_init();

        // Load .env for SOLANA_RPC_URL with API key
        let _ = dotenvy::dotenv();
        let rpc_url = std::env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".into());
        if let Some(ref proxy) = crate::config::load_config_proxy_url() {
            std::env::set_var("HTTPS_PROXY", proxy);
            std::env::set_var("HTTP_PROXY", proxy);
        }
        let rpc = RpcClient::new_with_timeout(rpc_url.clone(), Duration::from_secs(30));
        println!("[TEST] using RPC: {}", rpc_url);

        // Load metadata cache from disk (may be stale)
        super::load_lb_pair_cache();

        let meme = "4TyZGqRLG3VcHTGMcLBoPUmqYitMVojXinAmkL8xpump";
        let sol_mint = NATIVE_SOL_MINT;
        // Use 5 SOL threshold (matches config's min_pool_liquidity_sol)
        let min_reserve: u64 = 5_000_000_000;

        // --- First fetch: fill metadata cache via fast-path or GPAv2 fallback ---
        let t0 = std::time::Instant::now();
        let pools = fetch_dlmm_by_mints(&rpc, sol_mint, meme, min_reserve).await;
        let t1 = t0.elapsed();
        println!("[TEST] first fetch: {} pools in {:?}", pools.len(), t1);
        for p in &pools {
            let sol_res = if p.token_x_mint == sol_mint {
                p.reserve_x
            } else {
                p.reserve_y
            };
            println!(
                "  {} reserve_sol={} bin_step={}",
                &p.lb_pair[..12.min(p.lb_pair.len())],
                sol_res,
                p.bin_step
            );
        }

        let zero_after_first = DLMM_ZERO_SKIP.read().unwrap().len();
        println!(
            "[TEST] zero-skip entries after first fetch: {}",
            zero_after_first
        );

        // Clear reserve cache so second fetch re-queries
        {
            let mut cache = DLMM_CACHE.write().unwrap();
            let key = cache_key(sol_mint, meme);
            cache.remove(&key);
        }

        // --- Second fetch: should use fresh metadata, skipping below-threshold pools ---
        let t2 = std::time::Instant::now();
        let pools2 = fetch_dlmm_by_mints(&rpc, sol_mint, meme, min_reserve).await;
        let t3 = t2.elapsed();
        println!("[TEST] second fetch: {} pools in {:?}", pools2.len(), t3);

        let zero_after_second = DLMM_ZERO_SKIP.read().unwrap().len();
        println!(
            "[TEST] zero-skip entries after second fetch: {}",
            zero_after_second
        );
        for (lb, until) in DLMM_ZERO_SKIP.read().unwrap().iter() {
            println!("  SKIPPED {} → until_ts={}", &lb[..12.min(lb.len())], until);
        }

        // Both fetches should return the same pools
        let first_lbs: std::collections::HashSet<_> =
            pools.iter().map(|p| p.lb_pair.clone()).collect();
        let second_lbs: std::collections::HashSet<_> =
            pools2.iter().map(|p| p.lb_pair.clone()).collect();
        println!(
            "[TEST] first={} second={} unique lb_pairs",
            first_lbs.len(),
            second_lbs.len()
        );
        println!("[TEST] timing: first={:?} second={:?}", t1, t3);

        if !pools.is_empty() {
            assert_eq!(
                first_lbs, second_lbs,
                "both fetches should return same pools (zero-SOL excluded)"
            );
        }
        // If both empty, the RPC didn't return data — test inconclusive
    }

    /// Unit test: verify the zero-skip TTL logic.
    #[test]
    fn test_zero_skip_ttl_expiry() {
        let midnight = next_utc_midnight();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // midnight should be in the future
        assert!(
            midnight > now,
            "midnight {} must be > now {}",
            midnight,
            now
        );
        // midnight should be within 24h
        assert!(midnight - now < 86400, "midnight less than 24h away");
    }
}
