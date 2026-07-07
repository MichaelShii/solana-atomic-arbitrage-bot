//! Permanent metadata cache (SQLite)

use std::collections::HashMap;

use super::super::{DlmmPoolMetadata, DLMM_POOL_METADATA};
use super::DLMM_TOKEN_LBPAIR_CACHE;

pub(crate) fn save_metadata_cache() {
    let cache = match DLMM_POOL_METADATA.read() {
        Ok(c) => c,
        Err(_) => return,
    };
    let entries: Vec<_> = cache
        .iter()
        .map(|(k, v)| {
            let pools: Vec<(String, String, String, u16, u16, Option<String>)> = v
                .iter()
                .map(|m| {
                    (
                        m.lb_pair.clone(),
                        m.token_x_mint.clone(),
                        m.token_y_mint.clone(),
                        m.bin_step,
                        m.base_factor,
                        m.bin_array_bitmap_extension.clone(),
                    )
                })
                .collect();
            (k.clone(), pools)
        })
        .collect();
    crate::persistence::dlmm_metadata_replace_all(&entries);
}

pub(crate) fn load_metadata_cache() {
    let rows = crate::persistence::dlmm_metadata_load_all();
    if rows.is_empty() {
        log::debug!("[DLMM META] no metadata in DB");
        return;
    }
    let mut map: HashMap<String, Vec<DlmmPoolMetadata>> = HashMap::new();
    for row in &rows {
        map.entry(row.key.clone())
            .or_default()
            .push(DlmmPoolMetadata {
                lb_pair: row.lb_pair.clone(),
                token_x_mint: row.token_x_mint.clone(),
                token_y_mint: row.token_y_mint.clone(),
                bin_step: row.bin_step,
                base_factor: row.base_factor,
                bin_array_bitmap_extension: row.bin_array_bitmap_extension.clone(),
            });
    }
    let pool_count = rows.len();
    let mint_pair_count = map.len();
    if let Ok(mut cache) = DLMM_POOL_METADATA.write() {
        cache.extend(map);
    }
    log::info!(
        "[DLMM META] loaded {} pools across {} mint-pairs from DB",
        pool_count,
        mint_pair_count
    );
}

/// Load cached lb_pair mappings + metadata from SQLite at startup
pub fn load_lb_pair_cache() {
    let map = crate::persistence::lb_pairs_load_all();
    let count = map.len();
    if count > 0 {
        if let Ok(mut cache) = DLMM_TOKEN_LBPAIR_CACHE.write() {
            cache.extend(map);
        }
        log::info!("[LB_PAIR] loaded {} mint→lb_pair mappings from DB", count);
    } else {
        log::debug!("[LB_PAIR] no lb_pair cache in DB");
    }
    load_metadata_cache();
}

/// Persist new lb_pair to SQLite (incremental write)
fn save_lb_pair_cache(mint_a: &str, mint_b: &str, lb_pair: &str) {
    crate::persistence::lb_pairs_insert_both(mint_a, mint_b, lb_pair);
}

/// Populate the event-driven cache from a DLMM swap event.
/// Both input and output mints map to the same lb_pair.
/// Also persists to disk if the mapping is new.
pub fn cache_dlmm_lb_pair(mint_a: &str, mint_b: &str, lb_pair: &str) {
    let is_new;
    if let Ok(mut cache) = DLMM_TOKEN_LBPAIR_CACHE.write() {
        is_new = !cache.contains_key(mint_a);
        cache.insert(mint_a.to_string(), lb_pair.to_string());
        cache.insert(mint_b.to_string(), lb_pair.to_string());
    } else {
        return;
    }
    if is_new {
        log::info!(
            "[LB_PAIR] new lb_pair={} mint_a={} mint_b={}",
            &lb_pair[..lb_pair.len().min(12)],
            &mint_a[..mint_a.len().min(8)],
            &mint_b[..mint_b.len().min(8)],
        );
        save_lb_pair_cache(mint_a, mint_b, lb_pair);
    }
}
