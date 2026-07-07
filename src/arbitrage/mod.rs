//! Cross-venue arbitrage scanner
//!
//! Directly compares prices of the same token across multiple venues,
//! without relying on external transaction events.
//!
//! Primary path (from BOT_REPLICATION_PLAN.md):
//!   PumpSwap ↔ DLMM (64.7% of observed TXs)
//!
//! Flow: given mint → query multi-venue prices → compare spread → generate ArbitrageOpportunity

pub mod prices;
pub mod scanner;
pub mod scanner_queries;
pub mod types;

#[allow(unused_imports)]
use prices::PoolPriceSnapshot;
#[allow(unused_imports)]
pub use prices::VenuePrice;
pub use scanner::*;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(
        sol_per_token: f64,
        token_reserves: u64,
        sol_reserves: u64,
    ) -> PoolPriceSnapshot {
        PoolPriceSnapshot {
            sol_per_token,
            token_reserves_raw: token_reserves,
            sol_reserves_raw: sol_reserves,
            bins: vec![],
            meme_is_x: false,
            fee_bps: 25,
            lb_pair: String::new(),
            quote_decimals: 9,
        }
    }

    fn make_venue(venue: Venue, snapshots: Vec<PoolPriceSnapshot>) -> VenuePrice {
        let median = {
            let mut prices: Vec<f64> = snapshots.iter().map(|s| s.sol_per_token).collect();
            prices.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            prices.get(prices.len() / 2).copied()
        };
        VenuePrice {
            venue,
            sol_per_token: median,
            pool_prices: snapshots,
            token_reserves_raw: 0,
            sol_reserves_raw: 0,
            fee_bps: 25,
            bins: vec![],
            meme_is_x: false,
        }
    }

    #[test]
    fn test_venue_ordering() {
        let opp = ArbitrageOpportunity {
            signature: "test".into(),
            slot: 1,
            token_mint: "test".into(),
            buy_venue: Venue::PumpSwapAmm,
            sell_venue: Venue::MeteoraDlmm,
            buy_price_sol: 0.001,
            sell_price_sol: 0.0011,
            price_diff_bps: 1000,
            investment_sol: 1.0,
            expected_profit_sol: 0.1,
            net_profit_sol: 0.08,
            confidence: 0.8,
            dlmm_fee_bps: 25,
        };
        assert_eq!(opp.buy_venue.name(), "PumpSwap AMM");
        assert_eq!(opp.sell_venue.name(), "Meteora DLMM");
    }

    // ============================================================
    // Multi-pool best_buy / best_sell snapshot tests
    // ============================================================

    #[test]
    fn test_best_buy_picks_lowest_price() {
        let vp = make_venue(
            Venue::MeteoraDlmm,
            vec![
                make_snapshot(0.00100, 10_000_000, 10_000),
                make_snapshot(0.00090, 10_000_000, 9_000), // cheapest
                make_snapshot(0.00110, 10_000_000, 11_000),
            ],
        );
        let best = vp.best_buy_snapshot().unwrap();
        assert_eq!(best.sol_per_token, 0.00090);
    }

    #[test]
    fn test_best_sell_picks_highest_price() {
        let vp = make_venue(
            Venue::MeteoraDlmm,
            vec![
                make_snapshot(0.00100, 10_000_000, 10_000),
                make_snapshot(0.00120, 10_000_000, 12_000), // most expensive
                make_snapshot(0.00110, 10_000_000, 11_000),
            ],
        );
        let best = vp.best_sell_snapshot().unwrap();
        assert_eq!(best.sol_per_token, 0.00120);
    }

    #[test]
    fn test_dust_pool_is_skipped() {
        // Pool with < 1M token reserves should be filtered out
        let vp = make_venue(
            Venue::MeteoraDlmm,
            vec![
                make_snapshot(0.00050, 500_000, 250), // dust — skipped
                make_snapshot(0.00100, 10_000_000, 10_000),
                make_snapshot(0.00120, 10_000_000, 12_000),
            ],
        );
        // best_buy: dust pool at 0.00050 should NOT be picked (low reserves)
        let best_buy = vp.best_buy_snapshot().unwrap();
        assert_eq!(best_buy.sol_per_token, 0.00100);
        // best_sell: dust pool should not interfere
        let best_sell = vp.best_sell_snapshot().unwrap();
        assert_eq!(best_sell.sol_per_token, 0.00120);
    }

    #[test]
    fn test_all_dust_returns_none() {
        let vp = make_venue(
            Venue::MeteoraDlmm,
            vec![
                make_snapshot(0.00050, 500_000, 250),
                make_snapshot(0.00100, 999_999, 999),
            ],
        );
        assert!(vp.best_buy_snapshot().is_none());
        assert!(vp.best_sell_snapshot().is_none());
    }

    #[test]
    fn test_empty_pool_prices_returns_none() {
        let vp = make_venue(Venue::MeteoraDlmm, vec![]);
        assert!(vp.best_buy_snapshot().is_none());
        assert!(vp.best_sell_snapshot().is_none());
    }

    // ============================================================
    // Multi-pool estimate_profit: buy-cheap → sell-expensive
    // ============================================================

    #[test]
    fn test_estimate_profit_buys_from_cheapest_sells_to_priciest() {
        // Simulate: PumpSwap (1 pool) buy at 0.0010, DLMM (3 pools) sell at best=0.0012
        let scanner = ArbitrageScanner::new(
            0.0001,
            100,
            1.0,
            0.1,
            0.01,
            vec![Venue::PumpSwapAmm, Venue::MeteoraDlmm],
            0.000005,
            0.0001,
            1.0,
            100,
            false,
            0.02,
            2.0,
        );

        // Use realistic reserves: ~100 SOL per pool
        let buy_snap = make_snapshot(0.00100, 100_000_000_000, 100_000_000_000);
        let sell_snap = make_snapshot(0.00120, 100_000_000_000, 120_000_000_000);

        // With real reserves, CPMM formula yields positive profit
        let (investment, gross, net, confidence) =
            scanner.estimate_profit(&buy_snap, &sell_snap, 2000);
        assert!(
            investment > 0.0,
            "investment should be > 0, got {investment}"
        );
        assert!(
            gross > 0.0,
            "gross profit should be positive for buy-low sell-high, got {gross}"
        );
        assert!(confidence > 0.0, "confidence should be > 0");
        // net = gross - priority_fee - cu_cost
        assert!(net < gross, "net should be less than gross (fees deducted)");
    }

    #[test]
    fn test_estimate_profit_rejects_inverted_prices() {
        let scanner = ArbitrageScanner::new(
            0.0001,
            100,
            1.0,
            0.1,
            0.01,
            vec![Venue::PumpSwapAmm, Venue::MeteoraDlmm],
            0.000005,
            0.0001,
            1.0,
            100,
            false,
            0.02,
            2.0,
        );

        // buy price > sell price → no arbitrage direction, but estimate_profit still computes.
        // The caller (scan_inner) should have already filtered this out via buy < sell check.
        let buy_snap = make_snapshot(0.00120, 100_000_000_000, 120_000_000_000);
        let sell_snap = make_snapshot(0.00100, 100_000_000_000, 100_000_000_000);
        let (investment, gross, _net, _confidence) =
            scanner.estimate_profit(&buy_snap, &sell_snap, 2000);
        assert!(investment > 0.0);
        // buying high and selling low should yield negative or near-zero gross profit
        assert!(gross <= 0.0, "gross should be <= 0 when buy > sell");
    }

    #[test]
    fn test_estimate_profit_zero_reserves_returns_zero() {
        let scanner = ArbitrageScanner::new(
            0.0001,
            100,
            1.0,
            0.1,
            0.01,
            vec![],
            0.000005,
            0.0001,
            1.0,
            100,
            false,
            0.02,
            2.0,
        );
        let empty = make_snapshot(0.001, 0, 0);
        let (inv, gross, net, conf) = scanner.estimate_profit(&empty, &empty, 1000);
        assert_eq!(inv, 0.0);
        assert_eq!(gross, 0.0);
        assert_eq!(net, 0.0);
        assert_eq!(conf, 0.0);
    }

    #[test]
    fn test_estimate_profit_respects_max_investment_cap() {
        let scanner = ArbitrageScanner::new(
            0.0001,
            100,
            0.1,
            0.01,
            0.01,
            vec![],
            0.000005,
            0.0001,
            1.0,
            100,
            false,
            0.02,
            2.0,
        );
        // Very large pool — investment should be capped at max_investment (0.1 SOL)
        let buy_snap = make_snapshot(0.001, 1_000_000_000_000, 100_000_000_000);
        let sell_snap = make_snapshot(0.0012, 1_000_000_000_000, 120_000_000_000);
        let (investment, _gross, _net, _conf) =
            scanner.estimate_profit(&buy_snap, &sell_snap, 1000);
        assert!(
            investment <= 0.1 + 1e-9,
            "investment capped at max, got {}",
            investment
        );
    }

    // ── is_opportunity_fresh ──

    #[test]
    fn fresh_when_within_max_age() {
        assert!(is_opportunity_fresh(100, 110, 20));
        assert!(is_opportunity_fresh(100, 120, 20)); // exactly at boundary
    }

    #[test]
    fn stale_when_exceeds_max_age() {
        assert!(!is_opportunity_fresh(100, 121, 20));
        assert!(!is_opportunity_fresh(100, 200, 20));
    }

    #[test]
    fn snapshot_slot_zero_is_stale() {
        assert!(!is_opportunity_fresh(0, 100, 20));
        assert!(!is_opportunity_fresh(0, 0, 20));
    }

    #[test]
    fn current_slot_zero_is_stale() {
        // RPC failure → cur_slot=0 → snapshot can't be ahead, so stale
        assert!(!is_opportunity_fresh(100, 0, 20));
    }
}
