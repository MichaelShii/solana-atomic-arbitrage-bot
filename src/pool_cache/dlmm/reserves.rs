//! DLMM helper functions: stale-pair cleanup and fee-factor calculation.
//!
//! The main `fetch_dlmm_by_mints` lives in `discovery.rs`.

use solana_sdk::pubkey::Pubkey;

use super::super::{DlmmPoolReserves, DLMM_CACHE, DLMM_POOL_METADATA};
use super::{cache::save_metadata_cache, DLMM_TOKEN_LBPAIR_CACHE};

/// Clean up stale lb_pair mappings when the on-chain lb_pair account
/// has been deleted (closed by pool creator).
pub(super) fn cleanup_stale_lb_pairs(
    all_lb_pairs: &[Pubkey],
    all_reserves: &[Option<DlmmPoolReserves>],
) {
    let stale_lb_pairs: Vec<String> = all_lb_pairs
        .iter()
        .zip(all_reserves.iter())
        .filter(|(_, res)| res.is_none())
        .map(|(pair, _)| pair.to_string())
        .collect();

    if stale_lb_pairs.is_empty() {
        return;
    }

    log::warn!(
        "[DLMM] {} stale lb_pair(s) detected (account deleted on-chain), evicting from all caches",
        stale_lb_pairs.len(),
    );

    // Remove mint→lb_pair entries pointing to deleted accounts
    if let Ok(mut token_cache) = DLMM_TOKEN_LBPAIR_CACHE.write() {
        let before = token_cache.len();
        token_cache.retain(|mint, lb_pair| {
            if stale_lb_pairs.contains(lb_pair) {
                log::info!(
                    "[DLMM] evicting stale lb_pair={} mint={} (account deleted on-chain)",
                    &lb_pair[..lb_pair.len().min(12)],
                    &mint[..mint.len().min(8)],
                );
                false
            } else {
                true
            }
        });
        if token_cache.len() < before {
            let entries: Vec<(String, String)> = token_cache
                .iter()
                .map(|(m, lb)| (m.clone(), lb.clone()))
                .collect();
            drop(token_cache);
            crate::persistence::lb_pairs_replace_all(&entries);
        }
    }

    // Purge DLMM_CACHE entries whose cached reserves reference a deleted lb_pair
    {
        let mut cache = DLMM_CACHE.write().unwrap_or_else(|e| {
            log::warn!("DLMM cache poisoned, recovering");
            e.into_inner()
        });
        cache.retain(|_k, entry| match &entry.data {
            Some(reserves) => !stale_lb_pairs.contains(&reserves.lb_pair),
            None => true,
        });
    }

    // Purge DLMM_POOL_METADATA entries referencing deleted lb_pairs
    {
        if let Ok(mut mc) = DLMM_POOL_METADATA.write() {
            let before_pools: usize = mc.values().map(|v| v.len()).sum();
            mc.retain(|_key, vec| {
                vec.retain(|m| !stale_lb_pairs.contains(&m.lb_pair));
                !vec.is_empty()
            });
            let after_pools: usize = mc.values().map(|v| v.len()).sum();
            if after_pools < before_pools {
                log::info!(
                    "[DLMM] evicted {} stale pool(s) from metadata cache ({}→{})",
                    before_pools - after_pools,
                    before_pools,
                    after_pools,
                );
                drop(mc);
                save_metadata_cache();
            }
        }
    }
}

/// Return candidate baseFactor values for a given bin_step.
///
/// Based on fee formula: baseFactor = binStep * 10_000 / baseFeeBps
/// Common fee tiers: 25 bps (0.25%) and 100 bps (1%)
#[allow(dead_code)]
pub(super) fn base_factors_for(bin_step: u16) -> Vec<u16> {
    let step = bin_step as u32;
    let mut factors = vec![
        (step * 400) as u16, // 25 bps fee (most common)
        (step * 100) as u16, // 100 bps fee
    ];
    factors.dedup();
    factors
}
