//! ArbitrageScanner — scans prices across venues to find arbitrage opportunities.
//!
//! Directly compares prices of the same token across multiple venues,
//! without relying on external transaction events.

use log::debug;
use solana_client::nonblocking::rpc_client::RpcClient;

use super::types::{ArbitrageOpportunity, Venue};

// ============================================================
// ArbitrageScanner
// ============================================================

#[derive(Clone)]
pub struct ArbitrageScanner {
    pub min_profit_threshold_sol: f64,
    pub min_price_diff_bps: u32,
    pub max_investment_sol: f64,
    pub min_pool_tvl_sol: f64,
    #[allow(dead_code)]
    pub max_tip_sol: f64,
    pub enabled_venues: Vec<Venue>,
    pub cu_cost_sol: f64,
    pub profit_safety_factor: f64,
    pub pumpfun_fee_bps: u32,
    pub skip_reverify: bool,
    pub max_pool_share: f64,
    pub max_absolute_sol_out: f64,
}

impl ArbitrageScanner {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        min_profit_threshold_sol: f64,
        min_price_diff_bps: u32,
        max_investment_sol: f64,
        min_pool_tvl_sol: f64,
        max_tip_sol: f64,
        enabled_venues: Vec<Venue>,
        cu_cost_sol: f64,
        _priority_fee_sol: f64,
        profit_safety_factor: f64,
        pumpfun_fee_bps: u32,
        skip_reverify: bool,
        max_pool_share: f64,
        max_absolute_sol_out: f64,
    ) -> Self {
        Self {
            min_profit_threshold_sol,
            min_price_diff_bps,
            max_investment_sol,
            min_pool_tvl_sol,
            max_tip_sol,
            enabled_venues,
            cu_cost_sol,
            profit_safety_factor,
            pumpfun_fee_bps,
            skip_reverify,
            max_pool_share,
            max_absolute_sol_out,
        }
    }

    /// Scan by mint — shared entry point for scanner and watchlist scanning
    pub async fn scan_by_mint(&self, rpc: &RpcClient, mint: &str) -> Vec<ArbitrageOpportunity> {
        let gprc_slot = crate::grpc_stream::global_cache().latest_slot();
        let snapshot_slot = if gprc_slot > 0 {
            gprc_slot
        } else {
            rpc.get_slot_with_commitment(
                solana_sdk::commitment_config::CommitmentConfig::processed(),
            )
            .await
            .unwrap_or(0)
        };
        self.scan_inner(rpc, mint, "scanner", snapshot_slot).await
    }

    /// Pure RPC check for single venue (bypasses gRPC cache). Used exclusively for removal decisions, avoids progressive caching.
    #[allow(dead_code)]
    pub async fn is_single_venue_rpc(&self, rpc: &RpcClient, mint: &str) -> bool {
        // Call query_prices directly but force each venue to use RPC
        // Currently query_prices has an internal TTL cache, but pool_cache falls back to RPC
        let venue_prices = self.query_prices(rpc, mint).await;
        let count = venue_prices
            .iter()
            .filter(|p| p.sol_per_token.unwrap_or(0.0) > 0.0)
            .count();
        count < 2
    }

    /// Scan and return whether single venue (alongside opportunity list). Avoids two RPC calls from scan + check.
    #[allow(dead_code)]
    pub async fn scan_and_check_single(
        &self,
        rpc: &RpcClient,
        mint: &str,
    ) -> (Vec<ArbitrageOpportunity>, bool) {
        let gprc_slot = crate::grpc_stream::global_cache().latest_slot();
        let snapshot_slot = if gprc_slot > 0 {
            gprc_slot
        } else {
            rpc.get_slot_with_commitment(
                solana_sdk::commitment_config::CommitmentConfig::processed(),
            )
            .await
            .unwrap_or(0)
        };
        let venue_prices = self.query_prices(rpc, mint).await;
        let venue_count = venue_prices
            .iter()
            .filter(|p| p.sol_per_token.unwrap_or(0.0) > 0.0)
            .count();
        let opps = self.build_opps_from_prices(rpc, mint, "scanner", snapshot_slot, &venue_prices).await;
        (opps, venue_count < 2)
    }

    /// Core scanning logic: given a mint, query all venue prices and compare spreads
    async fn scan_inner(
        &self,
        rpc: &RpcClient,
        mint: &str,
        source: &str,
        slot: u64,
    ) -> Vec<ArbitrageOpportunity> {
        let venue_prices = self.query_prices(rpc, mint).await;
        self.log_venue_prices(&venue_prices, source, mint);
        self.build_opps_from_prices(rpc, mint, source, slot, &venue_prices)
            .await
    }

    fn log_venue_prices(
        &self,
        venue_prices: &[crate::arbitrage::prices::VenuePrice],
        source: &str,
        mint: &str,
    ) {
        let count = venue_prices.iter().filter(|vp| vp.sol_per_token.unwrap_or(0.0) > 0.0).count();
        if count < 2 {
            return; // single-venue — nothing to compare, skip logging
        }
        for vp in venue_prices {
            if let Some(p) = vp.sol_per_token {
                if p > 0.0 {
                    debug!(
                        "[SCAN] src={} venue={} mint={} sol_per_token={:.8}",
                        source,
                        vp.venue.name(),
                        &mint[..mint.len().min(8)],
                        p,
                    );
                }
            }
        }
    }

    /// Build opportunities from already-queried venue prices.
    async fn build_opps_from_prices(
        &self,
        rpc: &RpcClient,
        mint: &str,
        source: &str,
        slot: u64,
        venue_prices: &[crate::arbitrage::prices::VenuePrice],
    ) -> Vec<ArbitrageOpportunity> {
        let mut opportunities = Vec::new();

        let both_have_price = venue_prices
            .iter()
            .filter(|p| p.sol_per_token.unwrap_or(0.0) > 0.0)
            .count()
            >= 2;

        for buy_venue in &self.enabled_venues {
            let buy_price = match venue_prices.iter().find(|p| p.venue == *buy_venue) {
                Some(p) => p,
                None => continue,
            };
            let buy_snapshot = match buy_price.best_buy_snapshot() {
                Some(s) => s,
                None => continue,
            };
            let buy_sol_per_token = buy_snapshot.sol_per_token;
            let buy_tvl_sol = (buy_snapshot.sol_reserves_raw as f64
                / 10_f64.powi(buy_snapshot.quote_decimals as i32))
                * 2.0;
            if buy_tvl_sol < self.min_pool_tvl_sol {
                if both_have_price {
                    debug!(
                        "[FILTER] mint={} buy_tvl={:.3}SOL < min={:.1}SOL (venue={})",
                        &mint[..mint.len().min(12)],
                        buy_tvl_sol,
                        self.min_pool_tvl_sol,
                        buy_venue.name(),
                    );
                }
                continue;
            }

            for sell_venue in &self.enabled_venues {
                if buy_venue == sell_venue {
                    continue;
                }
                let sell_price = match venue_prices.iter().find(|p| p.venue == *sell_venue) {
                    Some(p) => p,
                    None => continue,
                };
                let sell_snapshot = match sell_price.best_sell_snapshot() {
                    Some(s) => s,
                    None => continue,
                };
                let sell_sol_per_token = sell_snapshot.sol_per_token;
                let sell_tvl_sol = (sell_snapshot.sol_reserves_raw as f64
                    / 10_f64.powi(sell_snapshot.quote_decimals as i32))
                    * 2.0;
                if sell_tvl_sol < self.min_pool_tvl_sol {
                    if both_have_price {
                        debug!(
                            "[FILTER] mint={} sell_tvl={:.3}SOL < min={:.1}SOL (venue={})",
                            &mint[..mint.len().min(12)],
                            sell_tvl_sol,
                            self.min_pool_tvl_sol,
                            sell_venue.name(),
                        );
                    }
                    continue;
                }

                if buy_sol_per_token >= sell_sol_per_token {
                    if both_have_price {
                        debug!(
                            "[FILTER] mint={} buy_price={:.8} >= sell_price={:.8} — no arb direction",
                            &mint[..mint.len().min(12)],
                            buy_sol_per_token,
                            sell_sol_per_token,
                        );
                    }
                    continue;
                }

                let price_diff = sell_sol_per_token - buy_sol_per_token;
                let price_diff_bps = ((price_diff / buy_sol_per_token) * 10000.0) as u32;

                if price_diff_bps < self.min_price_diff_bps {
                    if both_have_price {
                        debug!(
                            "[FILTER] mint={} diff={}bps < min={}bps (buy={:.8} sell={:.8})",
                            &mint[..mint.len().min(12)],
                            price_diff_bps,
                            self.min_price_diff_bps,
                            buy_sol_per_token,
                            sell_sol_per_token,
                        );
                    }
                    continue;
                }

                // Reject spreads > 5000bps (50%) — almost certainly data errors
                if price_diff_bps > 5000 {
                    if both_have_price {
                        debug!(
                            "[FILTER] mint={} absurd spread={}bps > 5000 — probable pricing error",
                            &mint[..mint.len().min(12)], price_diff_bps,
                        );
                    }
                    continue;
                }

                let (investment, gross_profit, net_profit, confidence) =
                    self.estimate_profit(buy_snapshot, sell_snapshot, price_diff_bps);

                // Re-fetch DLMM bins fresh before accepting any opportunity.
                // Stale cached bins can cause overestimated profit (H-05, M-07).
                // When skip_reverify is set, we skip this to save 80-200ms;
                // Phase C's TX build will still re-fetch fresh data.
                let dlmm_side = if !sell_snapshot.lb_pair.is_empty() {
                    Some((sell_snapshot, buy_snapshot, true))
                } else if !buy_snapshot.lb_pair.is_empty() {
                    Some((buy_snapshot, sell_snapshot, false))
                } else {
                    None
                };
                if let Some((dlmm_snap, other_snap, dlmm_is_sell)) = dlmm_side {
                    // Skip the re-fetch if gRPC cache has received data.
                    // A warm cache means pool state is already up-to-date, saving 80-500ms.
                    let gprc_warm = {
                        let s = crate::grpc_stream::global_cache().latest_slot();
                        s > 0
                    };
                    let should_reverify = !self.skip_reverify && !gprc_warm;
                    if net_profit >= self.min_profit_threshold_sol && should_reverify {
                        match self
                            .refetch_and_verify(
                                rpc,
                                dlmm_snap,
                                other_snap,
                                investment,
                                mint,
                                dlmm_is_sell,
                            )
                            .await
                        {
                            Ok(fresh_net) => {
                                if fresh_net < net_profit * 0.5 {
                                    debug!(
                                        "[STALE-RECHECK] mint={} original_net={:.6} fresh_net={:.6} — rejecting",
                                        &mint[..mint.len().min(12)], net_profit, fresh_net,
                                    );
                                    continue;
                                }
                                debug!(
                                    "[FRESH-VERIFIED] mint={} original_net={:.6} fresh_net={:.6}",
                                    &mint[..mint.len().min(12)],
                                    net_profit,
                                    fresh_net,
                                );
                            }
                            Err(e) => {
                                debug!(
                                    "[RECHECK-FAIL] mint={} error={}",
                                    &mint[..mint.len().min(12)],
                                    e
                                );
                                continue;
                            }
                        }
                    } else if net_profit >= self.min_profit_threshold_sol && gprc_warm {
                        log::debug!("[SKIP-REVERIFY] mint={} gRPC warm — trusting cached price", &mint[..mint.len().min(12)]);
                    }
                }

                // Sanity: gross should not exceed ~3× the theoretical max from the price spread.
                // Larger values are artifacts from pool-share caps binding on thin trades.
                let max_reasonable_gross = investment * (price_diff_bps as f64 / 10000.0) * 3.0;
                if gross_profit > max_reasonable_gross.max(0.001) {
                    debug!(
                        "[FILTER] mint={} unreal_profit invest={:.3} gross={:.6} spread={}bps max_reasonable={:.6} — cap artifact",
                        &mint[..mint.len().min(12)], investment, gross_profit, price_diff_bps, max_reasonable_gross,
                    );
                    continue;
                }

                // DEBUG: log raw reserves for profit outliers to diagnose overestimation
                if net_profit > 1.0 {
                    // Diagnose DLMM bin structure for sell-side outlier pools
                    let sell_bin_info = if !sell_snapshot.bins.is_empty() {
                        let total_sol: u64 = sell_snapshot
                            .bins
                            .iter()
                            .map(|b| {
                                if sell_snapshot.meme_is_x {
                                    b.amount_y
                                } else {
                                    b.amount_x
                                }
                            })
                            .sum();
                        let top3_sol: u64 = {
                            let mut sorted = sell_snapshot.bins.clone();
                            sorted.sort_by_key(|b| std::cmp::Reverse(b.bin_id));
                            sorted
                                .iter()
                                .take(3)
                                .map(|b| {
                                    if sell_snapshot.meme_is_x {
                                        b.amount_y
                                    } else {
                                        b.amount_x
                                    }
                                })
                                .sum()
                        };
                        let max_bin_sol = {
                            let mut sorted = sell_snapshot.bins.clone();
                            sorted.sort_by_key(|b| std::cmp::Reverse(b.bin_id));
                            sorted
                                .first()
                                .map(|b| {
                                    if sell_snapshot.meme_is_x {
                                        b.amount_y
                                    } else {
                                        b.amount_x
                                    }
                                })
                                .unwrap_or(0)
                        };
                        format!(
                            " bins={} total_sol={} top3_sol={} max_bin_sol={}",
                            sell_snapshot.bins.len(),
                            total_sol,
                            top3_sol,
                            max_bin_sol
                        )
                    } else {
                        String::new()
                    };
                    log::warn!(
                        "[PROFIT OUTLIER] mint={} buy_v={} buy_sol={} buy_tok={} | sell_v={} sell_sol={} sell_tok={} | invest={:.3} gross={:.6} net={:.6} diff={}bps{}",
                        &mint[..mint.len().min(12)],
                        buy_venue.name(),
                        buy_snapshot.sol_reserves_raw,
                        buy_snapshot.token_reserves_raw,
                        sell_venue.name(),
                        sell_snapshot.sol_reserves_raw,
                        sell_snapshot.token_reserves_raw,
                        investment,
                        gross_profit,
                        net_profit,
                        price_diff_bps,
                        sell_bin_info,
                    );
                }

                let safe_profit = net_profit * self.profit_safety_factor;
                if safe_profit <= 0.0 || safe_profit < self.min_profit_threshold_sol {
                    if both_have_price {
                        debug!(
                            "[FILTER] mint={} profit_reject invest={:.3} gross={:.6} net={:.6} safe={:.6} < min={:.6}",
                            &mint[..mint.len().min(12)],
                            investment,
                            gross_profit,
                            net_profit,
                            safe_profit,
                            self.min_profit_threshold_sol,
                        );
                    }
                    continue;
                }

                let dlmm_fee_bps = if *buy_venue == Venue::MeteoraDlmm {
                    buy_snapshot.fee_bps
                } else if *sell_venue == Venue::MeteoraDlmm {
                    sell_snapshot.fee_bps
                } else {
                    25
                };

                opportunities.push(ArbitrageOpportunity {
                    signature: source.to_string(),
                    slot,
                    token_mint: mint.to_string(),
                    buy_venue: *buy_venue,
                    sell_venue: *sell_venue,
                    buy_price_sol: buy_sol_per_token,
                    sell_price_sol: sell_sol_per_token,
                    price_diff_bps,
                    investment_sol: investment,
                    expected_profit_sol: gross_profit,
                    net_profit_sol: net_profit,
                    confidence,
                    dlmm_fee_bps,
                });
            }
        }

        opportunities.sort_by(|a, b| {
            b.net_profit_sol
                .partial_cmp(&a.net_profit_sol)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        opportunities
    }
}
