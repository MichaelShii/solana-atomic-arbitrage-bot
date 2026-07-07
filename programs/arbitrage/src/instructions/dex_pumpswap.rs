//! PumpSwap DEX handler — account validation and CPI orchestration.
//!
//! Accounts: 23 buy fixed or 21 sell fixed + remaining accounts.

use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult,
    pubkey::Pubkey,
};

use crate::{
    constants::*,
    cpi::pump_swap,
    error::{arb_err, ARB_BAD_PROGRAM, ARB_BAD_PDA},
};

/// Max PumpSwap buy section (23 fixed).
pub const PUMPSWAP_BUY_FIXED_LEN: usize = PUMP_BUY_FIXED_LEN;
/// PumpSwap sell section (21 fixed).
pub const PUMPSWAP_SELL_FIXED_LEN: usize = PUMP_SELL_FIXED_LEN;

/// Validate a PumpSwap section.
/// `base` is absolute index into accounts. `remaining` is remaining count.
pub fn validate_section(
    accounts: &[AccountInfo],
    base: usize,
    _remaining: usize,
) -> ProgramResult {
    // 1. Program ID check (index 16 for buy, 16 for sell — same offset)
    if accounts[base + PUMP_BUY_PROGRAM].key != &PUMP_SWAP_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // 2. Global config PDA check — index 2 (same for buy/sell)
    {
        let (expected, _) = Pubkey::find_program_address(
            &[PUMP_GLOBAL_CONFIG_SEED],
            &PUMP_SWAP_ID,
        );
        if accounts[base + PUMP_BUY_GLOBAL_CONFIG].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // 3. Event authority PDA — index 15 (same for buy/sell)
    {
        let (expected, _) = Pubkey::find_program_address(
            &[PUMP_EVENT_AUTH_SEED],
            &PUMP_SWAP_ID,
        );
        if accounts[base + PUMP_BUY_EVENT_AUTHORITY].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // Fee config PDA check skipped — buy (offset 21) vs sell (offset 19)
    // differ, and this validator handles both. The PumpSwap CPI will reject
    // a wrong fee_config on-chain.

    Ok(())
}

/// Build a PumpSwap buy CPI instruction (SOL → meme).
#[allow(clippy::too_many_arguments)]
pub fn build_buy_cpi(
    accounts: &[AccountInfo],
    pump_base: usize,
    remaining: usize,
    amount_in: u64,
    min_amount_out: u64,
    track_volume: bool,
    meme_ata_override: Option<usize>,
) -> solana_program::instruction::Instruction {
    pump_swap::build_buy(
        accounts, pump_base, remaining,
        amount_in, min_amount_out, track_volume,
        meme_ata_override,
    )
}

/// Build a PumpSwap sell CPI instruction (meme → SOL).
pub fn build_sell_cpi(
    accounts: &[AccountInfo],
    pump_base: usize,
    remaining: usize,
    amount_in: u64,
    min_amount_out: u64,
    meme_ata_override: Option<usize>,
) -> solana_program::instruction::Instruction {
    pump_swap::build_sell(
        accounts, pump_base, remaining,
        amount_in, min_amount_out,
        meme_ata_override,
    )
}
