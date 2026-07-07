//! Pre-swap PumpSwap pricing: read vault ATAs, compute CPMM output.

use solana_program::account_info::AccountInfo;
use solana_program::program_error::ProgramError;

use crate::accounting::read_token_amount;
use crate::error::{arb_err, ARB_PRICE_MATH_OVERFLOW};
use crate::pricing::cpmm_swap_output;

/// Estimate PumpSwap buy (SOL → meme) output from pool vault ATAs.
pub fn price_pumpswap_buy(
    pool_base_ata: &AccountInfo,
    pool_quote_ata: &AccountInfo,
    amount_in_lamports: u64,
    fee_bps: u16,
) -> Result<u64, ProgramError> {
    let quote_reserves = read_token_amount(pool_quote_ata)?;
    let base_reserves = read_token_amount(pool_base_ata)?;
    cpmm_swap_output(quote_reserves, base_reserves, amount_in_lamports, fee_bps)
        .map(|(out, _)| out)
        .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))
}

/// Estimate PumpSwap sell (meme → SOL) output from pool vault ATAs.
pub fn price_pumpswap_sell(
    pool_base_ata: &AccountInfo,
    pool_quote_ata: &AccountInfo,
    amount_in_tokens: u64,
    fee_bps: u16,
) -> Result<u64, ProgramError> {
    let base_reserves = read_token_amount(pool_base_ata)?;
    let quote_reserves = read_token_amount(pool_quote_ata)?;
    cpmm_swap_output(base_reserves, quote_reserves, amount_in_tokens, fee_bps)
        .map(|(out, _)| out)
        .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))
}
