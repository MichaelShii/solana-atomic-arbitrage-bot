//! Profit estimation and fresh-price verification methods for ArbitrageScanner.
//!
//! These methods are in a separate `impl` block to keep scanner.rs under 350 lines.


use solana_client::nonblocking::rpc_client::RpcClient;

use super::prices::{self, PoolPriceSnapshot, VenuePrice};
use super::scanner::ArbitrageScanner;
use super::types::Venue;

impl ArbitrageScanner {
    /// Query token prices across all venues (parallel query, reads gRPC cache directly, no local TTL)
    pub(super) async fn query_prices(&self, rpc: &RpcClient, mint: &str) -> Vec<VenuePrice> {
        let mint = mint.to_string();
        let min_reserve_lamports = crate::executor::atomic::helpers::sol_to_lamports(self.min_pool_tvl_sol);

        let futures: Vec<_> = self
            .enabled_venues
            .iter()
            .map(|venue| {
                let venue = *venue;
                let mint = mint.clone();
                Box::pin(async move {
                    match venue {
                        Venue::PumpSwapAmm => {
                            prices::query_pumpfun_price(rpc, &mint, self.pumpfun_fee_bps).await
                        }
                        Venue::MeteoraDlmm => {
                            prices::query_dlmm_price(rpc, &mint, min_reserve_lamports).await
                        }
                        Venue::RaydiumAmmv4 => prices::query_ammv4_price(rpc, &mint).await,
                        Venue::RaydiumCpmm => prices::query_cpmm_price(rpc, &mint).await,
                        Venue::OrcaWhirlpool => prices::query_whirlpool_price(rpc, &mint).await,
                    }
                })
            })
            .collect();

        futures::future::join_all(futures).await
    }

    /// Max fraction of a pool's reserves that can be traded in one swap.
    /// Absolute cap on SOL output per trade — defends against inflated DLMM bin sums.

    /// Estimate arbitrage profit (includes CPMM slippage, computed using on-chain raw reserve values)
    ///
    /// Buy (SOL → tokens): tokens = eff_sol_lamports × tok_reserve / (sol_reserve + eff_sol_lamports)
    /// Sell (tokens → SOL): sol_back = eff_tokens × sol_reserve_sell / (tok_reserve_sell + eff_tokens)
    /// Fees are deducted from the input side.
    ///
    /// Uses golden-section search to find the optimal investment amount (rather than fixed max_investment),
    /// finding the investment that maximizes net_profit within the [0, capped_max] interval.
    pub(crate) fn estimate_profit(
        &self,
        buy: &PoolPriceSnapshot,
        sell: &PoolPriceSnapshot,
        price_diff_bps: u32,
    ) -> (f64, f64, f64, f64) {
        const MIN_INVEST: f64 = 0.001;

        if buy.sol_reserves_raw == 0
            || buy.token_reserves_raw == 0
            || sell.sol_reserves_raw == 0
            || sell.token_reserves_raw == 0
        {
            return (0.0, 0.0, 0.0, 0.0);
        }

        // Quote unit: 1e9 for SOL, 1e6 for USDC
        let quote_unit = 10_f64.powi(buy.quote_decimals as i32);

        // Investment cap: quote-side 2% + token-side capacity + max_investment
        let quote_cap = (buy.sol_reserves_raw as f64 / quote_unit) * self.max_pool_share;
        let max_invest = quote_cap.min(self.max_investment_sol);
        if max_invest < MIN_INVEST {
            return (0.0, 0.0, 0.0, 0.0);
        }

        // Token capacity cap: at most buy 2% of the sell pool's tokens
        let max_tokens = (sell.token_reserves_raw as f64 * self.max_pool_share) as u128;
        if max_tokens == 0 {
            return (0.0, 0.0, 0.0, 0.0);
        }

        // Find the maximum investment satisfying the token capacity constraint (binary search)
        let upper = self.find_max_invest_within_capacity(buy, max_invest, max_tokens);
        if upper < MIN_INVEST {
            return (0.0, 0.0, 0.0, 0.0);
        }
        // Tighter bound: if we invest more than sell pool can return, it's a guaranteed loss
        // Sell pool max output is capped at sell_quote * MAX_POOL_SHARE in compute_arb_at_investment
        let max_out = (sell.sol_reserves_raw as f64 / quote_unit * self.max_pool_share)
            .min(self.max_absolute_sol_out);
        let upper = upper.min(max_out * 1.2); // allow 20% overhead for fees
        if upper < MIN_INVEST {
            return (0.0, 0.0, 0.0, 0.0);
        }

        // Golden-section search to find optimal investment
        let (best_invest, gross, net) = self.golden_search_profit(buy, sell, MIN_INVEST, upper);

        let confidence = {
            let price_confidence =
                ((price_diff_bps as f64 / self.min_price_diff_bps.max(1) as f64) - 1.0)
                    .clamp(0.0, 1.0);
            let depth_confidence = (best_invest / self.max_investment_sol).min(1.0);
            (price_confidence + depth_confidence) / 2.0
        };

        (best_invest, gross, net, confidence)
    }

    /// Binary search to find the maximum investment satisfying the token capacity constraint
    fn find_max_invest_within_capacity(
        &self,
        buy: &PoolPriceSnapshot,
        max_invest: f64,
        max_tokens: u128,
    ) -> f64 {
        // First test if the upper bound satisfies
        let tokens_at_max = self.simulate_buy(buy, max_invest);
        if tokens_at_max <= max_tokens {
            return max_invest;
        }
        // Binary search for maximum feasible investment
        let mut lo: f64 = 0.0;
        let mut hi: f64 = max_invest;
        for _ in 0..12 {
            let mid = (lo + hi) / 2.0;
            if self.simulate_buy(buy, mid) <= max_tokens {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Simulate buying: given SOL input, returns the token amount received
    fn simulate_buy(&self, buy: &PoolPriceSnapshot, investment: f64) -> u128 {
        let quote_unit = 10_f64.powi(buy.quote_decimals as i32);
        let quote_in = (investment * quote_unit) as u64;
        let fee_rate = buy.fee_bps as f64 / 10000.0;
        if !buy.bins.is_empty() {
            crate::simulator::estimate_dlmm_swap_output(&buy.bins, quote_in, !buy.meme_is_x, fee_rate)
                as u128
        } else {
            let eff = quote_in as u128 * (10000 - buy.fee_bps as u128) / 10000;
            (eff * buy.token_reserves_raw as u128) / (buy.sol_reserves_raw as u128 + eff)
        }
    }

    /// Given an investment amount, returns (investment, gross_profit, net_profit)
    fn compute_arb_at_investment(
        &self,
        buy: &PoolPriceSnapshot,
        sell: &PoolPriceSnapshot,
        investment: f64,
    ) -> (f64, f64) {
        let quote_unit = 10_f64.powi(buy.quote_decimals as i32);
        let quote_in = (investment * quote_unit) as u64;
        let buy_fee_rate = buy.fee_bps as f64 / 10000.0;
        let sell_fee_rate = sell.fee_bps as f64 / 10000.0;

        let tokens_got: u128 = if !buy.bins.is_empty() {
            crate::simulator::estimate_dlmm_swap_output(
                &buy.bins,
                quote_in,
                !buy.meme_is_x,
                buy_fee_rate,
            ) as u128
        } else {
            let eff_quote = quote_in as u128 * (10000 - buy.fee_bps as u128) / 10000;
            (eff_quote * buy.token_reserves_raw as u128) / (buy.sol_reserves_raw as u128 + eff_quote)
        };

        let quote_back_raw = if !sell.bins.is_empty() {
            crate::simulator::estimate_dlmm_swap_output(
                &sell.bins,
                tokens_got as u64,
                sell.meme_is_x,
                sell_fee_rate,
            ) as u128
        } else {
            let eff_tokens = tokens_got * (10000 - sell.fee_bps as u128) / 10000;
            (eff_tokens * sell.sol_reserves_raw as u128)
                / (sell.token_reserves_raw as u128 + eff_tokens)
        };

        // Cap output to 2% of sell pool's quote — symmetric with buy-side MAX_POOL_SHARE.
        // Without this, thin DLMM bins with concentrated quote can be "exhausted"
        // by the bin traversal, returning unrealistically high output.
        let pool_share_cap = (sell.sol_reserves_raw as f64 * self.max_pool_share) as u128;
        let quote_back = quote_back_raw.min(pool_share_cap);
        let gross = quote_back as f64 / quote_unit - investment;
        let net = gross - self.cu_cost_sol;
        (gross, net)
    }

    /// Golden-section search: find the investment that maximizes net_profit within [lo, hi]
    fn golden_search_profit(
        &self,
        buy: &PoolPriceSnapshot,
        sell: &PoolPriceSnapshot,
        lo: f64,
        hi: f64,
    ) -> (f64, f64, f64) {
        let phi: f64 = 1.618033988749895;
        let inv_phi = 1.0 / phi;
        let mut a = lo;
        let mut b = hi;
        let mut c = b - (b - a) * inv_phi;
        let mut d = a + (b - a) * inv_phi;

        let (_, mut fc) = self.compute_arb_at_investment(buy, sell, c);
        let (_, mut fd) = self.compute_arb_at_investment(buy, sell, d);

        for _ in 0..12 {
            if fc > fd {
                b = d;
                d = c;
                fd = fc;
                c = b - (b - a) * inv_phi;
                (_, fc) = self.compute_arb_at_investment(buy, sell, c);
            } else {
                a = c;
                c = d;
                fc = fd;
                d = a + (b - a) * inv_phi;
                (_, fd) = self.compute_arb_at_investment(buy, sell, d);
            }
        }

        let best = (a + b) / 2.0;
        let (gross, net) = self.compute_arb_at_investment(buy, sell, best);
        (best, gross, net)
    }

    /// Re-fetch a DLMM pool's bins fresh (bypassing cache) and re-compute profit.
    /// Returns the fresh net_profit estimate for the sell side, or an error if re-fetch fails.
    /// Re-fetch DLMM bins fresh and re-compute net profit.
    /// `dlmm_snap` is the DLMM-side snapshot, `other_snap` is the other venue.
    /// `dlmm_is_sell`: true if DLMM is the sell side, false if it's the buy side.
    pub(super) async fn refetch_and_verify(
        &self,
        rpc: &RpcClient,
        dlmm_snap: &PoolPriceSnapshot,
        other_snap: &PoolPriceSnapshot,
        investment: f64,
        mint: &str,
        dlmm_is_sell: bool,
    ) -> anyhow::Result<f64> {
        let fresh_bins = crate::pool_cache::fetch_bins_fresh(rpc, &dlmm_snap.lb_pair).await?;
        if fresh_bins.is_empty() {
            anyhow::bail!("no fresh bins found for {}", mint);
        }

        let dlmm_fee_rate = dlmm_snap.fee_bps as f64 / 10000.0;
        // Use other_snap's quote unit for the non-DLMM side (9 for SOL, 6 for USDC).
        let other_quote_unit = 10_f64.powi(other_snap.quote_decimals as i32);

        if dlmm_is_sell {
            // DLMM is sell side: buy via other venue (CPMM), sell via DLMM
            let (buy_snap, _sell_snap) = (other_snap, dlmm_snap);
            let quote_in = (investment * other_quote_unit) as u64;
            let eff_quote = quote_in as u128 * (10000 - buy_snap.fee_bps as u128) / 10000;
            let tokens_got: u128 = (eff_quote * buy_snap.token_reserves_raw as u128)
                / (buy_snap.sol_reserves_raw as u128 + eff_quote);

            let sol_back_raw = crate::simulator::estimate_dlmm_swap_output(
                &fresh_bins,
                tokens_got as u64,
                dlmm_snap.meme_is_x,
                dlmm_fee_rate,
            ) as u128;

            let max_sol_out = (dlmm_snap.sol_reserves_raw as f64 * self.max_pool_share)
                .min(self.max_absolute_sol_out * 1e9) as u128;
            let sol_back = sol_back_raw.min(max_sol_out);
            let gross = sol_back as f64 / 1e9 - investment; // DLMM returns SOL always
            let net = gross - self.cu_cost_sol;
            Ok(net)
        } else {
            // DLMM is buy side: buy via DLMM, sell via other venue (CPMM)
            let sol_in = (investment * 1e9) as u64; // DLMM buy always SOL
            let tokens_got = crate::simulator::estimate_dlmm_swap_output(
                &fresh_bins,
                sol_in,
                !dlmm_snap.meme_is_x,
                dlmm_fee_rate,
            ) as u128;

            let eff_tokens = tokens_got * (10000 - other_snap.fee_bps as u128) / 10000;
            let quote_back_raw = (eff_tokens * other_snap.sol_reserves_raw as u128)
                / (other_snap.token_reserves_raw as u128 + eff_tokens);

            let max_out = (other_snap.sol_reserves_raw as f64 * self.max_pool_share)
                .min(self.max_absolute_sol_out * other_quote_unit) as u128;
            let quote_back = quote_back_raw.min(max_out);
            let gross = quote_back as f64 / other_quote_unit - investment;
            let net = gross - self.cu_cost_sol;
            Ok(net)
        }
    }
}
