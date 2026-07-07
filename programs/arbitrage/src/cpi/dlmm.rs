//! DLMM `swap2` CPI helper.
//!
//! Constructs the `AccountMeta` vector in the exact swap2 order validated
//! against mainnet (R2-H02). The caller (route handler) passes absolute
//! indices for token mints, token programs, user ATAs, and bin arrays.
//!
//! ## Account layout (mirrored for route_dlmm_to_pump)
//!
//! When DLMM is the **second** leg (route_pump_to_dlmm), the DLMM-unique
//! accounts come after the PumpSwap section:
//!   dlmm_base = SHARED + PUMP_BUY_FIXED + pump_remaining
//!
//! When DLMM is the **first** leg (route_dlmm_to_pump), the DLMM-unique
//! accounts come right after the shared user accounts:
//!   dlmm_base = SHARED_FIXED_LEN

use alloc::vec::Vec;
use solana_program::{
    account_info::AccountInfo,
    instruction::{AccountMeta, Instruction},
};

use crate::constants::{
    DLMM_BIN_ARRAYS_START_REL, DLMM_BITMAP_REL, DLMM_EVENT_AUTH_REL, DLMM_HOST_FEE_REL,
    DLMM_LB_PAIR_REL, DLMM_MEMO_REL, DLMM_ORACLE_REL, DLMM_PROGRAM_REL, DLMM_RESERVE_X_REL,
    DLMM_RESERVE_Y_REL, DLMM_SWAP2_DISC,
};

/// Build a DLMM `swap2` CPI instruction.
///
/// `amount_in` is the exact token amount to swap. `min_amount_out` is
/// always 1 lamport for the second leg (the wrapping program's
/// `min_profit_lamports` invariant covers second-leg slippage).
///
/// Bin arrays are always writable (DLMM updates bin liquidity during swap).
#[allow(clippy::too_many_arguments)]
pub fn build_swap2(
    accounts: &[AccountInfo],
    dlmm_base: usize,
    amount_in: u64,
    // Absolute indices for token mints and programs (depend on route).
    token_x_mint_idx: usize,
    token_y_mint_idx: usize,
    token_x_program_idx: usize,
    token_y_program_idx: usize,
    // Absolute indices for user accounts.
    user_idx: usize,
    user_token_in_idx: usize,
    user_token_out_idx: usize,
    // Number of bin array accounts trailing the fixed DLMM section.
    bin_array_count: usize,
) -> Instruction {
    let mut meta: Vec<AccountMeta> = Vec::with_capacity(16 + bin_array_count);

    // swap2 AccountMeta order (verified against mainnet R2-H02):
    // 0: lb_pair (w)
    meta.push(acct(accounts, dlmm_base + DLMM_LB_PAIR_REL, true));
    // 1: bin_array_bitmap_extension (w — since DLMM v0.12.0 limit order logic)
    meta.push(acct(accounts, dlmm_base + DLMM_BITMAP_REL, true));
    // 2: reserve_x (w)
    meta.push(acct(accounts, dlmm_base + DLMM_RESERVE_X_REL, true));
    // 3: reserve_y (w)
    meta.push(acct(accounts, dlmm_base + DLMM_RESERVE_Y_REL, true));
    // 4: user_token_in (w)
    meta.push(acct(accounts, user_token_in_idx, true));
    // 5: user_token_out (w)
    meta.push(acct(accounts, user_token_out_idx, true));
    // 6: token_x_mint (r)
    meta.push(acct(accounts, token_x_mint_idx, false));
    // 7: token_y_mint (r)
    meta.push(acct(accounts, token_y_mint_idx, false));
    // 8: oracle (w)
    meta.push(acct(accounts, dlmm_base + DLMM_ORACLE_REL, true));
    // 9: host_fee_in (w)
    meta.push(acct(accounts, dlmm_base + DLMM_HOST_FEE_REL, true));
    // 10: user (r, signer)
    meta.push(acct(accounts, user_idx, false));
    // 11: token_x_program (r)
    meta.push(acct(accounts, token_x_program_idx, false));
    // 12: token_y_program (r)
    meta.push(acct(accounts, token_y_program_idx, false));
    // 13: memo_program (r)
    meta.push(acct(accounts, dlmm_base + DLMM_MEMO_REL, false));
    // 14: event_authority (r)
    meta.push(acct(accounts, dlmm_base + DLMM_EVENT_AUTH_REL, false));
    // 15: event_program (r) — DLMM program itself
    meta.push(acct(accounts, dlmm_base + DLMM_PROGRAM_REL, false));
    // 16..: bin arrays (writable — DLMM updates bin liquidity during swap)
    for i in 0..bin_array_count {
        meta.push(acct(
            accounts,
            dlmm_base + DLMM_BIN_ARRAYS_START_REL + i,
            true,
        ));
    }

    let mut data = Vec::with_capacity(28);
    data.extend_from_slice(&DLMM_SWAP2_DISC);
    data.extend_from_slice(&amount_in.to_le_bytes());
    // min_amount_out = 1 lamport. The wrapping program's min_profit_lamports
    // invariant already covers second-leg slippage; a tight min_out on the
    // inner CPI would only cause partial reverts.
    data.extend_from_slice(&1u64.to_le_bytes());
    // empty remaining_accounts_info (matches mainnet Swap2 CPI)
    data.extend_from_slice(&0u32.to_le_bytes());

    Instruction {
        program_id: *accounts[dlmm_base + DLMM_PROGRAM_REL].key,
        accounts: meta,
        data,
    }
}

#[inline]
fn acct(accounts: &[AccountInfo], idx: usize, writable: bool) -> AccountMeta {
    let a = &accounts[idx];
    if writable {
        AccountMeta::new(*a.key, a.is_signer)
    } else {
        AccountMeta::new_readonly(*a.key, a.is_signer)
    }
}
