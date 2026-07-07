//! Multi-venue price queries — query functions
//!
//! All price queries read from on-chain pool reserves — no external API pricing.

use solana_client::nonblocking::rpc_client::RpcClient;

use super::{PoolPriceSnapshot, VenuePrice};
use crate::arbitrage::Venue;
use crate::pool_cache;
use crate::simulator;

// DLMM price sanity bounds — corrupted active_id can produce astronomical prices.
// 1 SOL buys at most 1T tokens; 1 token costs at most 1M SOL.
pub(crate) const MIN_SANE_DLMM_PRICE: f64 = 1e-12;
pub(crate) const MAX_SANE_DLMM_PRICE: f64 = 1e6;

pub(crate) fn is_sane_dlmm_price(price: f64) -> bool {
    price.is_finite() && (MIN_SANE_DLMM_PRICE..=MAX_SANE_DLMM_PRICE).contains(&price)
}

// ============================================================
// Price queries
// ============================================================

/// Convert raw reserve ratio to SOL-per-token, accounting for decimal differences.
/// SOL always has 9 decimals; the token's decimals are guessed from known mints.
fn raw_to_sol_per_token(sol_raw: u64, token_raw: u64, token_mint: &str) -> Option<f64> {
    if token_raw == 0 {
        return None;
    }
    let token_decimals = crate::pool_cache::guess_decimals(token_mint);
    let ratio = sol_raw as f64 / token_raw as f64;
    // Adjust: if token has 6 decimals and SOL has 9, ratio is 1000× too high
    let adj = 10_f64.powi(9 - token_decimals as i32);
    Some(ratio / adj)
}

pub(crate) async fn query_pumpfun_price(rpc: &RpcClient, mint: &str, fee_bps: u32) -> VenuePrice {
    use pool_cache::PumpVenueKind;

    let state = pool_cache::fetch_bonding_curve(rpc, mint).await;
    match state {
        Some(ref s)
            if s.venue_kind == PumpVenueKind::PumpSwapPool
                && s.virtual_sol_reserves > 0
                && s.virtual_token_reserves > 0 =>
        {
            let sol_reserves = s.virtual_sol_reserves;
            let token_reserves = s.virtual_token_reserves;
            let sol_per_token = raw_to_sol_per_token(sol_reserves, token_reserves, mint);
            VenuePrice::from_single_pool(
                Venue::PumpSwapAmm,
                sol_per_token,
                token_reserves,
                sol_reserves,
                fee_bps,
                vec![],
                false,
                9,
            )
        }
        Some(ref s) if s.venue_kind == PumpVenueKind::BondingCurve => {
            // Bonding curves are not counted as executable cross-pool liquidity.
            // They remain available for discovery/migration monitoring via pool_cache.
            VenuePrice::from_single_pool(Venue::PumpSwapAmm, None, 0, 0, fee_bps, vec![], false, 9)
        }
        None | Some(_) => {
            VenuePrice::from_single_pool(Venue::PumpSwapAmm, None, 0, 0, fee_bps, vec![], false, 9)
        }
    }
}

pub(crate) fn compute_fee_bps(bin_step: u16, base_factor: u16) -> u32 {
    if base_factor == 0 {
        return 100; // unknown fee tier — use higher fee for conservative estimates
    }
    ((bin_step as u32 * 10000) / base_factor as u32).max(1)
}

pub(crate) async fn query_dlmm_price(
    rpc: &RpcClient,
    mint: &str,
    min_reserve_lamports: u64,
) -> VenuePrice {
    let sol_mint = "So11111111111111111111111111111111111111112";
    let no_price = || VenuePrice {
        venue: Venue::MeteoraDlmm,
        sol_per_token: None,
        pool_prices: vec![],
        token_reserves_raw: 0,
        sol_reserves_raw: 0,
        fee_bps: 25,
        bins: vec![],
        meme_is_x: false,
    };

    // Helper: build snapshots from DLMM pool records, optionally converting via sol_price()
    let build_snapshots = |pools: &[crate::pool_cache::DlmmPoolReserves],
                           quote_mint: &str,
                           decimals: u8,
                           sol_price: f64|
     -> Vec<PoolPriceSnapshot> {
        let mut snaps = Vec::with_capacity(pools.len());
        for r in pools {
            let meme_is_x = r.token_x_mint != quote_mint;
            let s = r.bin_step as f64 / 10000.0;
            let bin_price = (1.0 + s).powi(r.active_id);
            let quote_per_token = if meme_is_x { bin_price } else { 1.0 / bin_price };
            // Convert to SOL terms if quote is not SOL
            let sol_per_token = if decimals != 9 && sol_price > 0.0 {
                quote_per_token / sol_price
            } else {
                quote_per_token
            };

            if !is_sane_dlmm_price(sol_per_token) {
                continue;
            }

            let (total_x, total_y) = r.bins.iter().fold((0u64, 0u64), |(ax, ay), b| {
                (ax.saturating_add(b.amount_x), ay.saturating_add(b.amount_y))
            });
            // For USDC quote, the "sol_reserves" are USDC vault amounts; keep them for TVL
            let (token_reserves_raw, quote_reserves_raw) = if meme_is_x {
                (total_x, total_y)
            } else {
                (total_y, total_x)
            };

            snaps.push(PoolPriceSnapshot {
                sol_per_token,
                token_reserves_raw,
                sol_reserves_raw: quote_reserves_raw,
                bins: r.bins.clone(),
                meme_is_x,
                fee_bps: compute_fee_bps(r.bin_step, r.base_factor),
                lb_pair: r.lb_pair.clone(),
                quote_decimals: decimals,
            });
        }
        snaps
    };

    // 1. SOL-meme pairs
    let sol_pools =
        pool_cache::fetch_dlmm_by_mints(rpc, sol_mint, mint, min_reserve_lamports).await;
    let mut snapshots = build_snapshots(&sol_pools, sol_mint, 9, 1.0);

    // 2. USDC-meme pairs (merge with SOL, converting to SOL terms)
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let sol_price = crate::price::sol_price();
    if sol_price > 0.0 {
        let usdc_pools =
            pool_cache::fetch_dlmm_by_mints(rpc, usdc_mint, mint, min_reserve_lamports).await;
        if !usdc_pools.is_empty() {
            snapshots.extend(build_snapshots(&usdc_pools, usdc_mint, 6, sol_price));
        }
    }

    if snapshots.is_empty() {
        return no_price();
    }

    let median_price = {
        let mut prices: Vec<f64> = snapshots.iter().map(|s| s.sol_per_token).collect();
        prices.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        prices[prices.len() / 2]
    };

    let venue_fee = snapshots.first().map(|s| s.fee_bps).unwrap_or(25);

    VenuePrice {
        venue: Venue::MeteoraDlmm,
        sol_per_token: Some(median_price),
        pool_prices: snapshots,
        token_reserves_raw: 0,
        sol_reserves_raw: 0,
        fee_bps: venue_fee,
        bins: vec![],
        meme_is_x: false,
    }
}

pub(crate) async fn query_ammv4_price(rpc: &RpcClient, mint: &str) -> VenuePrice {
    let sol_mint = "So11111111111111111111111111111111111111112";
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    // Try SOL pool first, then USDC
    for (quote_mint, decimals) in [(sol_mint, 9u8), (usdc_mint, 6u8)] {
        let pool = pool_cache::get_ammv4_pool_info(quote_mint, mint);
        if let Some(info) = pool {
            if let Some((coin_vault, pc_vault)) =
                simulator::read_ammv4_vault_amounts(rpc, &info.coin_vault, &info.pc_vault).await
            {
                let quote_is_coin = info.coin_mint == quote_mint;
                let (quote_reserves, token_reserves) = if quote_is_coin {
                    (coin_vault, pc_vault)
                } else {
                    (pc_vault, coin_vault)
                };
                // Convert to SOL terms, adjusting for decimals
                let sol_per_token = if decimals == 6 {
                    // USDC quote: need USDC-specific decimal adjustment
                    let tok_dec = crate::pool_cache::guess_decimals(mint);
                    let usdc_per_token = quote_reserves as f64 / token_reserves as f64
                        / 10_f64.powi(6 - tok_dec as i32);
                    let sp = crate::price::sol_price();
                    if sp > 0.0 { Some(usdc_per_token / sp) } else { None }
                } else {
                    raw_to_sol_per_token(quote_reserves, token_reserves, mint)
                };
                return VenuePrice::from_single_pool(
                    Venue::RaydiumAmmv4,
                    sol_per_token,
                    token_reserves,
                    quote_reserves,
                    25, vec![], false, decimals,
                );
            }
        }
    }

    VenuePrice::from_single_pool(Venue::RaydiumAmmv4, None, 0, 0, 25, vec![], false, 9)
}

pub(crate) async fn query_cpmm_price(rpc: &RpcClient, mint: &str) -> VenuePrice {
    let sol_mint = "So11111111111111111111111111111111111111112";
    let no_price =
        || VenuePrice::from_single_pool(Venue::RaydiumCpmm, None, 0, 0, 25, vec![], false, 9);

    // 1. Try SOL-meme pair first (most common) — trigger PDA fetch if cache miss
    if pool_cache::get_pool_state(sol_mint, mint).is_none() {
        pool_cache::fetch_cpmm_now(rpc, sol_mint, mint).await;
    }
    if let Some(s) = pool_cache::get_pool_state(sol_mint, mint) {
        let (sol_reserves, token_reserves) = if s.token_0_mint == sol_mint {
            (s.token_0_vault_raw, s.token_1_vault_raw)
        } else {
            (s.token_1_vault_raw, s.token_0_vault_raw)
        };
        let sol_per_token = if token_reserves > 0 {
            raw_to_sol_per_token(sol_reserves, token_reserves, mint)
        } else {
            None
        };
        return VenuePrice::from_single_pool(
            Venue::RaydiumCpmm,
            sol_per_token,
            token_reserves,
            sol_reserves,
            25,
            vec![],
            false,
            9,
        );
    }

    // 2. Fallback: check discovered CPMM pools (non-PDA, from live tx logs).
    //    Trigger on-chain read via fetch_cpmm_by_address to get vault amounts.
    if let Some(pool_addr) = pool_cache::get_discovered_cpmm_pool(sol_mint, mint) {
        if let Some(s) = pool_cache::fetch_cpmm_by_address(rpc, &pool_addr).await {
            let (sol_reserves, token_reserves) = if s.token_0_mint == sol_mint {
                (s.token_0_vault_raw, s.token_1_vault_raw)
            } else {
                (s.token_1_vault_raw, s.token_0_vault_raw)
            };
            let sol_per_token = if token_reserves > 0 {
                raw_to_sol_per_token(sol_reserves, token_reserves, mint)
            } else {
                None
            };
            return VenuePrice::from_single_pool(
                Venue::RaydiumCpmm,
                sol_per_token,
                token_reserves,
                sol_reserves,
                25,
                vec![],
                false,
                9,
            );
        }
    }

    // 3. Fallback: find ANY CPMM pool containing this mint (scan entire cache)
    let s = match pool_cache::get_pool_state_by_mint(mint) {
        Some(s) => s,
        None => return no_price(),
    };

    // Determine which token is the meme and which is the quote
    let (meme_reserves_raw, quote_reserves_raw, quote_mint) = if s.token_0_mint == mint {
        (s.token_0_vault_raw, s.token_1_vault_raw, &s.token_1_mint)
    } else if s.token_1_mint == mint {
        (s.token_1_vault_raw, s.token_0_vault_raw, &s.token_0_mint)
    } else {
        return no_price();
    };

    // Convert to SOL only if quote is a known stablecoin (USDC/USDT)
    let sol_price = crate::price::sol_price();
    let (sol_per_token, quote_decimals) =
        if pool_cache::is_stablecoin(quote_mint) && sol_price > 0.0 {
            let meme_price_in_quote = quote_reserves_raw as f64 / meme_reserves_raw as f64;
            (Some(meme_price_in_quote / sol_price), 6)
        } else if quote_mint == sol_mint {
            // Shouldn't reach here (handled above), but be safe
            (Some(quote_reserves_raw as f64 / meme_reserves_raw as f64), 9)
        } else {
            (None, 9)
        };

    VenuePrice::from_single_pool(
        Venue::RaydiumCpmm,
        sol_per_token,
        meme_reserves_raw,
        quote_reserves_raw,
        25,
        vec![],
        false,
        quote_decimals,
    )
}

pub(crate) async fn query_whirlpool_price(rpc: &RpcClient, mint: &str) -> VenuePrice {
    let sol_mint = "So11111111111111111111111111111111111111112";
    let no_price =
        || VenuePrice::from_single_pool(Venue::OrcaWhirlpool, None, 0, 0, 25, vec![], false, 9);

    // 1. Try cache first
    if let Some(r) = pool_cache::get_whirlpool_reserves(sol_mint, mint) {
        let (sol_reserves, token_reserves) = if r.token_x_mint == sol_mint {
            (r.reserve_x, r.reserve_y)
        } else {
            (r.reserve_y, r.reserve_x)
        };
        return VenuePrice::from_single_pool(
            Venue::OrcaWhirlpool,
            if token_reserves > 0 {
                raw_to_sol_per_token(sol_reserves, token_reserves, mint)
            } else {
                None
            },
            token_reserves,
            sol_reserves,
            25,
            vec![],
            false,
            9,
        );
    }

    // 2. Trigger fetch via PDA derivation
    if pool_cache::fetch_whirlpool_by_mints(rpc, sol_mint, mint)
        .await
        .is_some()
    {
        if let Some(r) = pool_cache::get_whirlpool_reserves(sol_mint, mint) {
            let (sol_reserves, token_reserves) = if r.token_x_mint == sol_mint {
                (r.reserve_x, r.reserve_y)
            } else {
                (r.reserve_y, r.reserve_x)
            };
            return VenuePrice::from_single_pool(
                Venue::OrcaWhirlpool,
                if token_reserves > 0 {
                    raw_to_sol_per_token(sol_reserves, token_reserves, mint)
                } else {
                    None
                },
                token_reserves,
                sol_reserves,
                25,
                vec![],
                false,
                9,
            );
        }
    }

    // 3. Fallback: check discovered pools from WebSocket events
    if let Some(pool_addr) = pool_cache::get_discovered_whirlpool_pool(sol_mint, mint) {
        if let Some(r) = pool_cache::fetch_whirlpool_by_address(rpc, &pool_addr).await {
            let (sol_reserves, token_reserves) = if r.token_x_mint == sol_mint {
                (r.reserve_x, r.reserve_y)
            } else {
                (r.reserve_y, r.reserve_x)
            };
            return VenuePrice::from_single_pool(
                Venue::OrcaWhirlpool,
                if token_reserves > 0 {
                    raw_to_sol_per_token(sol_reserves, token_reserves, mint)
                } else {
                    None
                },
                token_reserves,
                sol_reserves,
                25,
                vec![],
                false,
                9,
            );
        }
    }

    // 4. USDC fallback: try all three paths with USDC as quote
    let usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    let sol_price = crate::price::sol_price();
    if sol_price > 0.0 {
        // 4a. Cache
        if let Some(r) = pool_cache::get_whirlpool_reserves(usdc_mint, mint) {
            let (usdc_reserves, token_reserves) = if r.token_x_mint == usdc_mint {
                (r.reserve_x, r.reserve_y)
            } else {
                (r.reserve_y, r.reserve_x)
            };
            return VenuePrice::from_single_pool(
                Venue::OrcaWhirlpool,
                if token_reserves > 0 {
                    let usdc_per_token = usdc_reserves as f64 / token_reserves as f64;
                    Some(usdc_per_token / sol_price)
                } else { None },
                token_reserves,
                usdc_reserves,
                25, vec![], false, 6,
            );
        }
        // 4b. PDA fetch
        if pool_cache::fetch_whirlpool_by_mints(rpc, usdc_mint, mint).await.is_some() {
            if let Some(r) = pool_cache::get_whirlpool_reserves(usdc_mint, mint) {
                let (usdc_reserves, tk) = if r.token_x_mint == usdc_mint {
                    (r.reserve_x, r.reserve_y)
                } else {
                    (r.reserve_y, r.reserve_x)
                };
                return VenuePrice::from_single_pool(
                    Venue::OrcaWhirlpool,
                    if tk > 0 { Some(usdc_reserves as f64 / tk as f64 / sol_price) } else { None },
                    tk, usdc_reserves, 25, vec![], false, 6,
                );
            }
        }
        // 4c. Discovered pools
        if let Some(pool_addr) = pool_cache::get_discovered_whirlpool_pool(usdc_mint, mint) {
            if let Some(r) = pool_cache::fetch_whirlpool_by_address(rpc, &pool_addr).await {
                let (usdc_reserves, tk) = if r.token_x_mint == usdc_mint {
                    (r.reserve_x, r.reserve_y)
                } else {
                    (r.reserve_y, r.reserve_x)
                };
                return VenuePrice::from_single_pool(
                    Venue::OrcaWhirlpool,
                    if tk > 0 { Some(usdc_reserves as f64 / tk as f64 / sol_price) } else { None },
                    tk, usdc_reserves, 25, vec![], false, 6,
                );
            }
        }
    }

    no_price()
}
