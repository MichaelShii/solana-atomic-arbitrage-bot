//! Multi-venue price data structures
//!
//! Defines VenuePrice and PoolPriceSnapshot — the per-venue price data
//! used by the arbitrage scanner for cross-venue comparison.
//!
//! Query functions (query_pumpfun_price, query_dlmm_price, etc.) live in price_query.rs
//! and are re-exported through this module to keep call-sites unchanged.

mod price_query;
pub(crate) use price_query::*;

use super::Venue;
use crate::pool_cache::DlmmBin;

// ============================================================
// Venue price snapshot
// ============================================================

/// Per-pool price data for multi-pool venues (DLMM).
/// When a venue has multiple pools for the same mint-pair, each pool gets its own snapshot.
#[derive(Debug, Clone)]
pub struct PoolPriceSnapshot {
    pub sol_per_token: f64,
    pub token_reserves_raw: u64,
    pub sol_reserves_raw: u64,
    pub bins: Vec<DlmmBin>,
    pub meme_is_x: bool,
    pub fee_bps: u32,
    /// DLMM lb_pair address (empty for non-DLMM venues). Used for fresh bin re-fetch.
    pub lb_pair: String,
    /// Quote token decimals: 9 for SOL, 6 for USDC. Used for raw amount → UI conversion.
    pub quote_decimals: u8,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VenuePrice {
    pub venue: Venue,
    /// Best representative single price (backward compat). For multi-pool venues,
    /// this is the median pool's price.
    pub sol_per_token: Option<f64>,
    /// Multi-pool: each pool's price and liquidity data. Empty for single-pool venues.
    pub pool_prices: Vec<PoolPriceSnapshot>,
    pub token_reserves_raw: u64,
    pub sol_reserves_raw: u64,
    pub fee_bps: u32,
    pub bins: Vec<DlmmBin>,
    pub meme_is_x: bool,
}

impl VenuePrice {
    /// Get the pool snapshot with the lowest price (for buying)
    pub fn best_buy_snapshot(&self) -> Option<&PoolPriceSnapshot> {
        self.pool_prices
            .iter()
            .filter(|s| s.token_reserves_raw >= 1_000_000 && s.sol_per_token.is_finite())
            .min_by(|a, b| {
                a.sol_per_token
                    .partial_cmp(&b.sol_per_token)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Get the pool snapshot with the highest price (for selling)
    pub fn best_sell_snapshot(&self) -> Option<&PoolPriceSnapshot> {
        self.pool_prices
            .iter()
            .filter(|s| s.token_reserves_raw >= 1_000_000 && s.sol_per_token.is_finite())
            .max_by(|a, b| {
                a.sol_per_token
                    .partial_cmp(&b.sol_per_token)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    pub(crate) fn from_single_pool(
        venue: Venue,
        sol_per_token: Option<f64>,
        token_reserves_raw: u64,
        sol_reserves_raw: u64,
        fee_bps: u32,
        bins: Vec<DlmmBin>,
        meme_is_x: bool,
        quote_decimals: u8,
    ) -> Self {
        let snapshot = sol_per_token
            .filter(|&p| p > 0.0)
            .map(|p| PoolPriceSnapshot {
                sol_per_token: p,
                token_reserves_raw,
                sol_reserves_raw,
                bins: bins.clone(),
                meme_is_x,
                fee_bps,
                lb_pair: String::new(),
                quote_decimals,
            });
        Self {
            venue,
            sol_per_token,
            pool_prices: snapshot.into_iter().collect(),
            token_reserves_raw,
            sol_reserves_raw,
            fee_bps,
            bins,
            meme_is_x,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::price_query::{
        compute_fee_bps, is_sane_dlmm_price, MAX_SANE_DLMM_PRICE, MIN_SANE_DLMM_PRICE,
    };

    // ── compute_fee_bps ──

    #[test]
    fn fee_25bps_bin_step_1_base_400() {
        assert_eq!(compute_fee_bps(1, 400), 25);
    }

    #[test]
    fn fee_100bps_bin_step_1_base_100() {
        assert_eq!(compute_fee_bps(1, 100), 100);
    }

    #[test]
    fn fee_25bps_bin_step_100_base_40000() {
        assert_eq!(compute_fee_bps(100, 40000), 25);
    }

    #[test]
    fn unknown_base_factor_falls_back_to_100bps() {
        assert_eq!(compute_fee_bps(100, 0), 100);
        assert_eq!(compute_fee_bps(1, 0), 100);
    }

    #[test]
    fn fee_at_least_1bp() {
        assert_eq!(compute_fee_bps(1, 20000), 1);
    }

    #[test]
    fn fee_rounds_down() {
        assert_eq!(compute_fee_bps(50, 20000), 25);
    }

    // ── is_sane_dlmm_price ──

    #[test]
    fn normal_price_is_sane() {
        assert!(is_sane_dlmm_price(1.0));
        assert!(is_sane_dlmm_price(0.000001)); // 1e-6 — typical meme coin
        assert!(is_sane_dlmm_price(100.0)); // expensive token
    }

    #[test]
    fn boundary_price_is_sane() {
        assert!(is_sane_dlmm_price(MIN_SANE_DLMM_PRICE));
        assert!(is_sane_dlmm_price(MAX_SANE_DLMM_PRICE));
    }

    #[test]
    fn nan_is_insane() {
        assert!(!is_sane_dlmm_price(f64::NAN));
    }

    #[test]
    fn infinity_is_insane() {
        assert!(!is_sane_dlmm_price(f64::INFINITY));
        assert!(!is_sane_dlmm_price(f64::NEG_INFINITY));
    }

    #[test]
    fn astronomical_price_is_insane() {
        // Simulates corrupted active_id producing huge prices
        assert!(!is_sane_dlmm_price(1e10));
        assert!(!is_sane_dlmm_price(1e100));
        assert!(!is_sane_dlmm_price(f64::MAX));
    }

    #[test]
    fn tiny_price_is_insane() {
        // Simulates corrupted active_id producing near-zero prices
        assert!(!is_sane_dlmm_price(1e-15));
        assert!(!is_sane_dlmm_price(1e-100));
        assert!(!is_sane_dlmm_price(0.0));
    }

    #[test]
    fn negative_price_is_insane() {
        assert!(!is_sane_dlmm_price(-1.0));
        assert!(!is_sane_dlmm_price(-0.001));
    }
}
