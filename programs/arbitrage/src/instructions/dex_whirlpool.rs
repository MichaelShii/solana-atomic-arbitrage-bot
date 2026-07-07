//! Orca Whirlpool DEX handler — account validation and CPI orchestration.
//!
//! Accounts: 12 fixed + 0-3 tick arrays.
//!   [0]=Whirlpool program, [1]=token_authority(PDA), [2]=whirlpool state,
//!   [3]=user input ATA, [4]=vault_a, [5]=user output ATA, [6]=vault_b,
//!   [7-9]=tick arrays, [10]=oracle, [11]=token_program
//! Verified against official Orca Whirlpool Codama IDL (2026-06).

use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult,
    pubkey::Pubkey,
};

use crate::{
    constants::*,
    cpi::whirlpool,
    error::{arb_err, ARB_BAD_ACCOUNT_COUNT, ARB_BAD_PDA, ARB_BAD_PROGRAM},
};

/// Full Whirlpool section: 12 fixed + tick_array_count additional.
pub const WHIRLPOOL_FIXED_LEN: usize = 12;
/// Tick arrays start at offset 7 from whirlpool_base.
pub const WHIRLPOOL_TICK_START: usize = 7;
/// Token program position for CPI (Orca swap expects token_program at meta[0]).
pub const WHIRLPOOL_TOKEN_PROG_OFF: usize = 11;

/// Validate the Whirlpool section accounts.
///
/// `whirlpool_base` is the absolute index where the Whirlpool section starts.
/// `tick_array_count` is the number of tick array accounts (1-3, even though
/// the IDL defines exactly 3 in the fixed section; additional ones go as
/// remaining).
pub fn validate_section(
    accounts: &[AccountInfo],
    whirlpool_base: usize,
    tick_array_count: usize,
) -> ProgramResult {
    // 1. Program ID check (index 0)
    if accounts[whirlpool_base].key != &WHIRLPOOL_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // 2. Tick array count bounds
    if tick_array_count < 1 || tick_array_count > 3 {
        return Err(arb_err(ARB_BAD_ACCOUNT_COUNT));
    }

    // 3. PDA: token_authority = PDA("authority") — index 1
    {
        let (expected, _) =
            Pubkey::find_program_address(&[WHIRLPOOL_AUTH_SEED], &WHIRLPOOL_ID);
        if accounts[whirlpool_base + 1].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // 4. Whirlpool state owned by WHIRLPOOL_ID — index 2
    if accounts[whirlpool_base + 2].owner != &WHIRLPOOL_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // 5. Oracle PDA: [b"oracle", whirlpool_address] — index 10
    {
        let whirlpool_key = accounts[whirlpool_base + 2].key;
        let (expected, _) = Pubkey::find_program_address(
            &[b"oracle", whirlpool_key.as_ref()],
            &WHIRLPOOL_ID,
        );
        if accounts[whirlpool_base + 10].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // 6. Token program: must match a known SPL token program — index 11
    if accounts[whirlpool_base + 11].key != &TOKEN_ID
        && accounts[whirlpool_base + 11].key != &Pubkey::new_from_array(
            solana_program::pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").to_bytes())
    {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    Ok(())
}

/// Build a Whirlpool swap instruction via the CPI builder.
#[allow(clippy::too_many_arguments)]
pub fn build_swap_cpi(
    accounts: &[AccountInfo],
    whirlpool_base: usize,
    tick_array_count: usize,
    amount_in: u64,
    min_amount_out: u64,
    a_to_b: bool,
    token_prog_a_idx: usize,
    token_prog_b_idx: usize,
    input_ata_idx: usize,
    output_ata_idx: usize,
) -> solana_program::instruction::Instruction {
    whirlpool::build_swap(
        accounts,
        whirlpool_base,
        tick_array_count,
        amount_in,
        min_amount_out,
        a_to_b,
        token_prog_a_idx,
        token_prog_b_idx,
        input_ata_idx,
        output_ata_idx,
    )
}
