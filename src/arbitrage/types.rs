//! Core types for cross-venue arbitrage scanning.

// ============================================================
// Types
// ============================================================

/// Trading venue
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Venue {
    PumpSwapAmm,
    MeteoraDlmm,
    RaydiumAmmv4,
    RaydiumCpmm,
    OrcaWhirlpool,
}

impl Venue {
    pub fn name(&self) -> &'static str {
        match self {
            Venue::PumpSwapAmm => "PumpSwap AMM",
            Venue::MeteoraDlmm => "Meteora DLMM",
            Venue::RaydiumAmmv4 => "Raydium AMMv4",
            Venue::RaydiumCpmm => "Raydium CPMM",
            Venue::OrcaWhirlpool => "Orca Whirlpool",
        }
    }
}

/// Cross-pool arbitrage opportunity
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ArbitrageOpportunity {
    pub signature: String,
    pub slot: u64,
    pub token_mint: String,
    pub buy_venue: Venue,
    pub sell_venue: Venue,
    pub buy_price_sol: f64,
    pub sell_price_sol: f64,
    pub price_diff_bps: u32,
    pub investment_sol: f64,
    pub expected_profit_sol: f64,
    pub net_profit_sol: f64,
    pub confidence: f64,
    pub dlmm_fee_bps: u32,
}

/// Pure helper: true if the opportunity's snapshot slot is recent enough.
/// `snapshot_slot` is the slot captured before scanning; `current_slot` from RPC/gRPC.
/// `max_age` in slots (≈400ms each). Returns false for `snapshot_slot == 0`.
/// When gRPC stream is active, pool data comes in real-time — freshness check is skipped.
pub fn is_opportunity_fresh(snapshot_slot: u64, current_slot: u64, max_age: u64) -> bool {
    if snapshot_slot == 0 || current_slot == 0 {
        return false;
    }
    // gRPC stream provides real-time data → skip freshness check
    if crate::grpc_stream::global_cache().latest_slot() > 0 {
        return true;
    }
    current_slot.saturating_sub(snapshot_slot) <= max_age
}
