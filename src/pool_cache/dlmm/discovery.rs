//! DLMM pool discovery + fetch (main entry point)
//!
//! Discovers all DLMM pools for a given mint pair and fetches their reserves.

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::super::{
    cache_key, next_utc_midnight, CacheEntry, DlmmPoolMetadata, DlmmPoolReserves, DLMM_CACHE,
    DLMM_POOL_METADATA, DLMM_RESERVE_TTL_SECS, DLMM_ZERO_SKIP,
};
use super::{
    bins::{fetch_reserves_inner, fetch_reserves_with_metadata},
    cache::{cache_dlmm_lb_pair, save_metadata_cache},
    gpa::discover_lb_pairs_via_gpa,
    reserves::cleanup_stale_lb_pairs,
    DLMM_TOKEN_LBPAIR_CACHE,
};
use crate::constants::DLMM_PROGRAM;

/// DLMM pool discovery + reserve fetching
///
/// Strategy (by priority):
///   1. Short-TTL reserve cache (10s dedup)
///   2. Permanent metadata → fast fresh fetch (2 RPCs, ~400ms)
///   3. Event-driven token→lb_pair cache (in-memory)
///   4. GPAv2 discovery via getProgramAccountsV2 (2 parallel, 1 credit each)
pub async fn fetch_dlmm_by_mints(
    rpc: &RpcClient,
    sol_mint: &str,
    meme_mint: &str,
    min_reserve_lamports: u64,
) -> Vec<DlmmPoolReserves> {
    let key = cache_key(sol_mint, meme_mint);

    // Check short-lived reserve cache (10s TTL for dedup).
    {
        let cache = match DLMM_CACHE.read() {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        if let Some(entry) = cache.get(&key) {
            if entry.age_secs() < DLMM_RESERVE_TTL_SECS {
                return match entry.data.clone() {
                    Some(r) => vec![r],
                    None => vec![],
                };
            }
        }
    }

    // Fast path: try metadata-based fresh fetch for cached pools.
    // Skip pools whose SOL reserves were below threshold last fetch (cooldown until next UTC midnight).
    let now_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let cached_metas: Vec<DlmmPoolMetadata> = {
        let skip = DLMM_ZERO_SKIP.read().ok();
        DLMM_POOL_METADATA
            .read()
            .ok()
            .and_then(|mc| mc.get(&key).cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|m| {
                skip.as_ref()
                    .and_then(|s| s.get(&m.lb_pair))
                    .map(|&until| now_ts >= until)
                    .unwrap_or(true) // not in skip list → query
            })
            .collect()
    };
    if !cached_metas.is_empty() {
        let futures: Vec<_> = cached_metas
            .iter()
            .map(|meta| fetch_reserves_with_metadata(rpc, meta))
            .collect();
        let results: Vec<Option<DlmmPoolReserves>> = futures::future::join_all(futures).await;

        let mut dead_pools: Vec<String> = Vec::new();
        let mut below_threshold: Vec<String> = Vec::new();
        let mut live_results: Vec<DlmmPoolReserves> = Vec::new();
        for (meta, res) in cached_metas.iter().zip(results) {
            match res {
                Some(ref r) => {
                    let sol_res = if r.token_x_mint == sol_mint {
                        r.reserve_x
                    } else {
                        r.reserve_y
                    };
                    if sol_res < min_reserve_lamports {
                        below_threshold.push(meta.lb_pair.clone());
                    }
                    live_results.push(r.clone());
                }
                None => dead_pools.push(meta.lb_pair.clone()),
            }
        }

        // Mark below-threshold pools in skip list for 30 minutes
        if !below_threshold.is_empty() {
            if let Ok(mut skip) = DLMM_ZERO_SKIP.write() {
                let until = chrono::Utc::now().timestamp() + 1800;
                for lb in &below_threshold {
                    skip.insert(lb.clone(), until);
                }
            }
        }

        // Evict dead pools from metadata
        if !dead_pools.is_empty() {
            if let Ok(mut mc) = DLMM_POOL_METADATA.write() {
                if let Some(entry) = mc.get_mut(&key) {
                    entry.retain(|m| !dead_pools.contains(&m.lb_pair));
                    if entry.is_empty() {
                        mc.remove(&key);
                    }
                    drop(mc);
                    save_metadata_cache();
                }
            }
        }

        // Filter out below-threshold pools for actual use
        let usable: Vec<DlmmPoolReserves> = live_results
            .into_iter()
            .filter(|r| {
                let sol_res = if r.token_x_mint == sol_mint {
                    r.reserve_x
                } else {
                    r.reserve_y
                };
                sol_res >= min_reserve_lamports
            })
            .collect();

        if !usable.is_empty() {
            let best = usable.iter().max_by_key(|r| {
                if r.token_x_mint == sol_mint {
                    r.reserve_x
                } else {
                    r.reserve_y
                }
            });
            {
                let mut cache = DLMM_CACHE.write().unwrap_or_else(|e| {
                    log::warn!("DLMM cache poisoned, recovering");
                    e.into_inner()
                });
                cache.insert(key.clone(), CacheEntry::new(best.cloned()));
            }
            return usable;
        }
        // All cached pools are dead or below threshold — fall through to full discovery
    }

    let dlmm = match Pubkey::from_str(DLMM_PROGRAM) {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    // Collect ALL lb_pair addresses for this token pair (dedup by pubkey)
    let mut all_lb_pairs: Vec<Pubkey> = Vec::new();

    // ---- Strategy 1: Event-driven cache ----
    if let Some(lb_pair_str) = DLMM_TOKEN_LBPAIR_CACHE
        .read()
        .ok()
        .and_then(|cache| cache.get(meme_mint).cloned())
    {
        if let Ok(lb_pair_pk) = Pubkey::from_str(&lb_pair_str) {
            all_lb_pairs.push(lb_pair_pk);
        }
    }

    let mut new_lb_pairs: Vec<Pubkey> = Vec::new();

    // ---- Strategy 2: GPAv2 discovery (replaces token-account + PDA scan) ----
    // Two parallel getProgramAccountsV2 calls (1 credit each) with memcmp on both, limit=5000, limit=5000
    // token mints + dataSize=904. Covers all lb_pair types: permissionless,
    // permissioned, preset, and factory-migrated.
    let (gpa_a, gpa_b) = tokio::join!(
        discover_lb_pairs_via_gpa(sol_mint, meme_mint),
        discover_lb_pairs_via_gpa(meme_mint, sol_mint),
    );
    for (pk, _active_id, _bin_step, _base_factor, _bitmap_ext) in gpa_a.into_iter().chain(gpa_b) {
        if !all_lb_pairs.contains(&pk) {
            all_lb_pairs.push(pk);
            new_lb_pairs.push(pk);
        }
    }

    // Persist newly discovered lb_pairs to the event-driven cache + disk
    if !new_lb_pairs.is_empty() {
        let lb_pair_str = new_lb_pairs[0].to_string();
        cache_dlmm_lb_pair(sol_mint, meme_mint, &lb_pair_str);
    }

    if all_lb_pairs.is_empty() {
        let mut cache = DLMM_CACHE.write().unwrap_or_else(|e| {
            log::warn!("DLMM cache poisoned, recovering");
            e.into_inner()
        });
        cache.insert(key, CacheEntry::new_negative(None));

        log::debug!(
            "[DLMM] no pool found for mint={} (GPAv2 returned no matching lb_pairs, negative-cached 15s)",
            &meme_mint[..meme_mint.len().min(8)],
        );
        return vec![];
    }

    // Fetch reserves for ALL discovered lb_pairs in parallel
    let futures: Vec<_> = all_lb_pairs
        .iter()
        .map(|lb_pair_pk| fetch_reserves_inner(rpc, lb_pair_pk, &dlmm))
        .collect();
    let all_reserves: Vec<Option<DlmmPoolReserves>> = futures::future::join_all(futures).await;

    // Clean up stale lb_pair → mint mappings when the on-chain lb_pair account
    // has been deleted (closed by pool creator). Without this, the bot keeps
    // computing spreads against phantom pools that no longer exist.
    cleanup_stale_lb_pairs(&all_lb_pairs, &all_reserves);

    // Filter to SOL-denominated pools only; mark below-threshold pools in skip list.
    let (below_threshold, valid_reserves): (Vec<DlmmPoolReserves>, Vec<DlmmPoolReserves>) =
        all_reserves
            .into_iter()
            .flatten()
            .filter(|r| r.token_x_mint == sol_mint || r.token_y_mint == sol_mint)
            .partition(|r| {
                let sol_res = if r.token_x_mint == sol_mint {
                    r.reserve_x
                } else {
                    r.reserve_y
                };
                sol_res < min_reserve_lamports
            });

    if !below_threshold.is_empty() {
        if let Ok(mut skip) = DLMM_ZERO_SKIP.write() {
            let midnight = next_utc_midnight();
            for r in &below_threshold {
                skip.insert(r.lb_pair.clone(), midnight);
            }
        }
        log::debug!(
            "[DLMM] mint={} {} below-threshold pool(s) marked for skip until midnight (min={} lamports)",
            &meme_mint[..meme_mint.len().min(8)],
            below_threshold.len(),
            min_reserve_lamports,
        );
    }

    if valid_reserves.is_empty() {
        let mut cache = DLMM_CACHE.write().unwrap_or_else(|e| {
            log::warn!("DLMM cache poisoned, recovering");
            e.into_inner()
        });
        cache.insert(key, CacheEntry::new_negative(None));
        return vec![];
    }

    log::debug!(
        "[DLMM] mint={} found {} pools, top SOL reserve={}",
        &meme_mint[..meme_mint.len().min(8)],
        valid_reserves.len(),
        valid_reserves
            .iter()
            .map(|r| {
                if r.token_x_mint == sol_mint {
                    r.reserve_x
                } else {
                    r.reserve_y
                }
            })
            .max()
            .unwrap_or(0),
    );

    // Cache the best pool for short-TTL dedup
    {
        let best = valid_reserves.iter().max_by_key(|r| {
            if r.token_x_mint == sol_mint {
                r.reserve_x
            } else {
                r.reserve_y
            }
        });
        if let Some(best) = best {
            let mut cache = DLMM_CACHE.write().unwrap_or_else(|e| {
                log::warn!("DLMM cache poisoned, recovering");
                e.into_inner()
            });
            cache.insert(key.clone(), CacheEntry::new(Some(best.clone())));
        }
    }
    // Persist ALL valid pools to metadata
    {
        let all_metas: Vec<DlmmPoolMetadata> = valid_reserves
            .iter()
            .map(|r| DlmmPoolMetadata {
                lb_pair: r.lb_pair.clone(),
                token_x_mint: r.token_x_mint.clone(),
                token_y_mint: r.token_y_mint.clone(),
                bin_step: r.bin_step,
                base_factor: r.base_factor,
                bin_array_bitmap_extension: r.bin_array_bitmap_extension.clone(),
            })
            .collect();
        if let Ok(mut mc) = DLMM_POOL_METADATA.write() {
            mc.insert(key, all_metas);
            drop(mc);
            save_metadata_cache();
        }
    }
    valid_reserves
}
