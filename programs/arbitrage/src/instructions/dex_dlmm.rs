//! Meteora DLMM DEX handler — account validation and CPI orchestration.

use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult,
    pubkey::Pubkey,
};

use crate::{
    constants::*,
    cpi::dlmm,
    error::{arb_err, ARB_BAD_ACCOUNT_COUNT, ARB_BAD_PROGRAM, ARB_BAD_PDA},
};

/// DLMM section: 9 base + 4 extra (mints/programs) + bin_arrays.
/// Layout: [0]=program, [1]=lb_pair, [2]=bitmap, [3]=reserve_x,
/// [4]=reserve_y, [5]=oracle, [6]=host_fee, [7]=memo, [8]=event_auth,
/// [9]=token_x_mint, [10]=token_y_mint, [11]=token_x_prog, [12]=token_y_prog.
pub const DLMM_FIXED_LEN: usize = 13;
/// Token mint/program offsets for generic orchestrator CPI.
pub const DLMM_TOKEN_X_MINT_OFF: usize = 9;
pub const DLMM_TOKEN_Y_MINT_OFF: usize = 10;
pub const DLMM_TOKEN_X_PROG_OFF: usize = 11;
pub const DLMM_TOKEN_Y_PROG_OFF: usize = 12;

/// Validate a DLMM section.
pub fn validate_section(
    accounts: &[AccountInfo],
    base: usize,
    remaining: usize,
) -> ProgramResult {
    // 0. Bin array count bounds (1-6, matches DLMM IDL). 0 would be a client bug.
    if remaining < 1 || remaining > 6 {
        return Err(arb_err(ARB_BAD_ACCOUNT_COUNT));
    }
    // 1. Program ID check (index 0 of DLMM section = DLMM_PROGRAM_REL)
    if accounts[base + DLMM_PROGRAM_REL].key != &DLMM_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // 2. LbPair owned by DLMM program (index 1)
    if accounts[base + DLMM_LB_PAIR_REL].owner != &DLMM_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // 3. Oracle PDA: [b"oracle", lb_pair_address] — index 5
    {
        let lb_pair = accounts[base + DLMM_LB_PAIR_REL].key;
        let (expected, _) = Pubkey::find_program_address(
            &[DLMM_ORACLE_SEED, lb_pair.as_ref()],
            &DLMM_ID,
        );
        if accounts[base + DLMM_ORACLE_REL].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // 4. Memo program at index 7
    if accounts[base + DLMM_MEMO_REL].key != &MEMO_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // 5. Event authority at index 8
    if accounts[base + DLMM_EVENT_AUTH_REL].key != &DLMM_EVENT_AUTH {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    Ok(())
}

/// Build a DLMM swap2 CPI instruction.
#[allow(clippy::too_many_arguments)]
pub fn build_swap2_cpi(
    accounts: &[AccountInfo],
    dlmm_base: usize,
    amount_in: u64,
    token_x_mint_idx: usize,
    token_y_mint_idx: usize,
    token_x_program_idx: usize,
    token_y_program_idx: usize,
    user_idx: usize,
    user_token_in_idx: usize,
    user_token_out_idx: usize,
    bin_array_count: usize,
) -> solana_program::instruction::Instruction {
    dlmm::build_swap2(
        accounts, dlmm_base,
        amount_in,
        token_x_mint_idx, token_y_mint_idx,
        token_x_program_idx, token_y_program_idx,
        user_idx, user_token_in_idx, user_token_out_idx,
        bin_array_count,
    )
}
