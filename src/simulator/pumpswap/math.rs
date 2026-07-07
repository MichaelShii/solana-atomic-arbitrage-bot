use super::{checked_estimate_swap_output, estimate_swap_output};

// ============================================================
// Estimation functions (constant-product AMM)
// ============================================================

/// Estimate PumpSwap AMM buy output given constant-product reserves.
/// `fee_bps` is the protocol fee in basis points (default 25 = 0.25%).
#[allow(dead_code)]
pub fn estimate_pumpswap_buy_output(
    quote_amount_in: u64,
    quote_reserves: u64,
    base_reserves: u64,
    fee_bps: u32,
) -> u64 {
    estimate_swap_output(
        quote_reserves,
        base_reserves,
        quote_amount_in,
        fee_bps as f64 / 10000.0,
    )
}

/// Estimate PumpSwap AMM sell output (SOL received for tokens).
#[allow(dead_code)]
pub fn estimate_pumpswap_sell_output(
    base_amount_in: u64,
    base_reserves: u64,
    quote_reserves: u64,
    fee_bps: u32,
) -> u64 {
    estimate_swap_output(
        base_reserves,
        quote_reserves,
        base_amount_in,
        fee_bps as f64 / 10000.0,
    )
}

// ============================================================
// Checked variants (u128, overflow-safe) — for PumpSwap on-chain CPI
// ============================================================

/// Checked PumpSwap buy estimation. Returns `None` if the on-chain constant
/// product would overflow u128 or produce zero output.
pub fn checked_pumpswap_buy_output(
    quote_amount_in: u64,
    quote_reserves: u64,
    base_reserves: u64,
    fee_bps: u32,
) -> Option<u64> {
    checked_estimate_swap_output(quote_reserves, base_reserves, quote_amount_in, fee_bps)
}

/// Checked PumpSwap sell estimation. Returns `None` if the on-chain constant
/// product would overflow u128 or produce zero output.
pub fn checked_pumpswap_sell_output(
    base_amount_in: u64,
    base_reserves: u64,
    quote_reserves: u64,
    fee_bps: u32,
) -> Option<u64> {
    checked_estimate_swap_output(base_reserves, quote_reserves, base_amount_in, fee_bps)
}
