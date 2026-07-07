//! Raydium CPMM swap CPI helper.
//!
//! Builds the `AccountMeta` vector in the exact order specified by the
//! Raydium CPMM IDL (swap instruction, 13 fixed accounts).

use alloc::vec::Vec;
use solana_program::{
    account_info::AccountInfo,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::constants::CPMM_SWAP_DISC;

// ── CPMM swap: 13 fixed accounts ───────────────────────────────────
//
//  0: payer            (signer, writable)
//  1: authority        (readonly — PDA("vault_and_lp", pool_state))
//  2: amm_config       (readonly)
//  3: pool_state       (writable)
//  4: input_token_acct (writable — user's input ATA)
//  5: output_token_acct(writable — user's output ATA)
//  6: input_vault      (writable — pool input token vault)
//  7: output_vault     (writable — pool output token vault)
//  8: input_mint       (readonly)
//  9: output_mint      (readonly)
// 10: input_token_prog (readonly — Tokenkeg or Token-2022)
// 11: output_token_prog(readonly)
// 12: memo_program     (readonly)

pub const CPMM_FIXED_LEN: usize = 13;

/// Build a CPMM `swap` CPI instruction.
///
/// `amount_in` is the exact input amount. `min_amount_out` is the
/// minimum output (second-leg invariant handled by the orchestrator).
///
/// All indices are absolute into the full `accounts` slice.
#[allow(clippy::too_many_arguments)]
pub fn build_swap(
    accounts: &[AccountInfo],
    cpmm_base: usize,
    amount_in: u64,
    min_amount_out: u64,
    // Absolute indices
    payer_idx: usize,
    input_mint_idx: usize,
    output_mint_idx: usize,
    input_ata_idx: usize,
    output_ata_idx: usize,
    input_token_prog_idx: usize,
    output_token_prog_idx: usize,
    memo_program_idx: usize,
) -> Instruction {
    let mut meta: Vec<AccountMeta> = Vec::with_capacity(13);

    // 0: payer (user wallet, signer, writable)
    meta.push(acct(accounts, payer_idx, true));
    // 1: authority (PDA — validated on-chain)
    meta.push(acct(accounts, cpmm_base + 1, false));
    // 2: amm_config (readonly)
    meta.push(acct(accounts, cpmm_base + 2, false));
    // 3: pool_state (writable)
    meta.push(acct(accounts, cpmm_base + 3, true));
    // 4: input_token_account (user's input ATA)
    meta.push(acct(accounts, input_ata_idx, true));
    // 5: output_token_account (user's output ATA)
    meta.push(acct(accounts, output_ata_idx, true));
    // 6: input_vault (pool vault)
    meta.push(acct(accounts, cpmm_base + 6, true));
    // 7: output_vault (pool vault)
    meta.push(acct(accounts, cpmm_base + 7, true));
    // 8: input_mint
    meta.push(acct(accounts, input_mint_idx, false));
    // 9: output_mint
    meta.push(acct(accounts, output_mint_idx, false));
    // 10: input_token_program
    meta.push(acct(accounts, input_token_prog_idx, false));
    // 11: output_token_program
    meta.push(acct(accounts, output_token_prog_idx, false));
    // 12: memo_program
    meta.push(acct(accounts, memo_program_idx, false));

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&CPMM_SWAP_DISC);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());

    Instruction {
        program_id: *accounts[cpmm_base].key,
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
