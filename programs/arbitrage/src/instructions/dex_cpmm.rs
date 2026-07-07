//! Raydium CPMM DEX handler — account validation and CPI orchestration.
//!
//! Provides `validate_section` for the generic orchestrator. The CPMM
//! swap uses 13 fixed accounts (see `cpi::cpmm` for layout).

use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult,
    pubkey::Pubkey,
};

use crate::{
    constants::*,
    cpi::cpmm,
    error::{arb_err, ARB_BAD_PDA, ARB_BAD_PROGRAM},
};

/// Full CPMM section: 13 fixed accounts (no remaining).
pub const CPMM_FIXED_LEN: usize = 13;

/// Validate the CPMM section accounts.
///
/// `cpmm_base` is the absolute index where the CPMM section starts
/// in the full account list. The section must have exactly `CPMM_FIXED_LEN`
/// accounts.
pub fn validate_section(
    accounts: &[AccountInfo],
    cpmm_base: usize,
    _remaining_count: usize,
) -> ProgramResult {
    // 1. Program ID check
    if accounts[cpmm_base].key != &CPMM_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // 2. amm_config well-known check
    if accounts[cpmm_base + 2].key != &CPMM_AMM_CONFIG {
        // Accept other configs too — CPMM uses multiple amm_configs.
        // Just verify it's a valid account.
    }

    // 3. Token/memo program checks (indices 10, 11, 12)
    let token22 = Pubkey::new_from_array(
        solana_program::pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").to_bytes());
    let in_tok_prog = accounts[cpmm_base + 10].key;
    let out_tok_prog = accounts[cpmm_base + 11].key;
    if *in_tok_prog != TOKEN_ID && *in_tok_prog != token22 {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }
    if *out_tok_prog != TOKEN_ID && *out_tok_prog != token22 {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }
    if accounts[cpmm_base + 12].key != &MEMO_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // 4. PDA: authority = PDA("vault_and_lp", pool_state)
    {
        let pool_state = accounts[cpmm_base + 3].key;
        let (expected, _) =
            Pubkey::find_program_address(&[CPMM_AUTH_SEED, pool_state.as_ref()], &CPMM_ID);
        if accounts[cpmm_base + 1].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // 5. Ensure the pool state account is owned by CPMM
    if accounts[cpmm_base + 3].owner != &CPMM_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    Ok(())
}

/// Build a CPMM swap instruction via the CPI builder.
///
/// Returns the instruction to be invoked.
#[allow(clippy::too_many_arguments)]
pub fn build_swap_cpi(
    accounts: &[AccountInfo],
    cpmm_base: usize,
    amount_in: u64,
    min_amount_out: u64,
    payer_idx: usize,
    input_mint_idx: usize,
    output_mint_idx: usize,
    input_ata_idx: usize,
    output_ata_idx: usize,
    input_token_prog_idx: usize,
    output_token_prog_idx: usize,
    memo_program_idx: usize,
) -> solana_program::instruction::Instruction {
    cpmm::build_swap(
        accounts,
        cpmm_base,
        amount_in,
        min_amount_out,
        payer_idx,
        input_mint_idx,
        output_mint_idx,
        input_ata_idx,
        output_ata_idx,
        input_token_prog_idx,
        output_token_prog_idx,
        memo_program_idx,
    )
}
