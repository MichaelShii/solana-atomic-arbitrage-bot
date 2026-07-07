//! Orca Whirlpool swap CPI helper (verified against official IDL 2026-06).
//!
//! Account order from the Orca Whirlpool Codama-generated IDL:
//!   [0] token_program    (readonly)
//!   [1] token_authority   (readonly, PDA signer — seeds: [b"authority"])
//!   [2] whirlpool         (writable — pool state)
//!   [3] token_owner_acct_a(writable — user input ATA)
//!   [4] token_vault_a     (writable — pool vault A)
//!   [5] token_owner_acct_b(writable — user output ATA)
//!   [6] token_vault_b     (writable — pool vault B)
//!   [7] tick_array_0      (writable)
//!   [8] tick_array_1      (writable)
//!   [9] tick_array_2      (writable)
//!  [10] oracle            (readonly — PDA: [b"oracle", whirlpool])

use alloc::vec::Vec;
use solana_program::{
    account_info::AccountInfo,
    instruction::{AccountMeta, Instruction},
};

use crate::constants::WHIRLPOOL_SWAP_DISC;

/// Full whirlpool section: 11 fixed + N additional tick arrays.
pub const WHIRLPOOL_FIXED_LEN: usize = 11;
/// Index of the first tick array relative to whirlpool_base.
pub const WHIRLPOOL_TICK_START: usize = 7;

/// Build an Orca Whirlpool `swap` CPI instruction.
///
/// The `token_authority` at `whirlpool_base + 1` is a PDA signer
/// derived from `["authority"]`. The caller must provide the seeds.
///
/// All indices are absolute into the full `accounts` slice.
#[allow(clippy::too_many_arguments)]
pub fn build_swap(
    accounts: &[AccountInfo],
    whirlpool_base: usize,
    tick_array_count: usize,
    amount_in: u64,
    min_amount_out: u64,
    a_to_b: bool,
    // Absolute indices
    token_prog_a_idx: usize,
    token_prog_b_idx: usize,
    input_ata_idx: usize,
    output_ata_idx: usize,
) -> Instruction {
    let mut meta: Vec<AccountMeta> = Vec::with_capacity(11 + tick_array_count);

    // 0: token_program (defaults to Tokenkeg)
    meta.push(AccountMeta::new_readonly(*accounts[token_prog_a_idx].key, false));
    // 1: token_authority (PDA signer — seeds from caller)
    meta.push(AccountMeta::new_readonly(*accounts[whirlpool_base + 1].key, true));
    // 2: whirlpool (pool state, writable)
    meta.push(AccountMeta::new(*accounts[whirlpool_base + 2].key, false));
    // 3: token_owner_acct_a (user input ATA)
    meta.push(AccountMeta::new(*accounts[input_ata_idx].key, false));
    // 4: token_vault_a (pool vault A)
    meta.push(AccountMeta::new(*accounts[whirlpool_base + 4].key, false));
    // 5: token_owner_acct_b (user output ATA)
    meta.push(AccountMeta::new(*accounts[output_ata_idx].key, false));
    // 6: token_vault_b (pool vault B)
    meta.push(AccountMeta::new(*accounts[whirlpool_base + 6].key, false));
    // 7-9: tick_arrays (writable)
    for i in 0..tick_array_count.min(3) {
        meta.push(AccountMeta::new(
            *accounts[whirlpool_base + 7 + i].key,
            false,
        ));
    }
    // 10: oracle (PDA: [b"oracle", whirlpool_address], readonly)
    meta.push(AccountMeta::new_readonly(*accounts[whirlpool_base + 10].key, false));

    // Instruction data (42 bytes):
    let mut data = Vec::with_capacity(42);
    data.extend_from_slice(&WHIRLPOOL_SWAP_DISC);           // [0..8]
    data.extend_from_slice(&amount_in.to_le_bytes());        // [8..16]
    data.extend_from_slice(&min_amount_out.to_le_bytes());   // [16..24]
    // sqrt_price_limit: max u128 for buy (no limit), 0 for sell. Safe because the
    // orchestrator already enforces min_amount_out via post-invariants.
    let price_limit: u128 = if a_to_b { u128::MAX } else { 0 };
    data.extend_from_slice(&price_limit.to_le_bytes());      // [24..40]
    data.push(1u8);  // amount_specified_is_input = true      // [40]
    data.push(a_to_b as u8);                                 // [41]

    Instruction {
        program_id: *accounts[whirlpool_base].key,
        accounts: meta,
        data,
    }
}
