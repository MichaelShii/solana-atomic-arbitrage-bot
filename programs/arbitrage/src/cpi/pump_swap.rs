//! PumpSwap AMM CPI helpers: `buy_exact_quote_in` and `sell`.
//!
//! Each function builds the `AccountMeta` vector and instruction data
//! matching the official PumpSwap IDL layout.

use alloc::vec::Vec;
use solana_program::{
    account_info::AccountInfo,
    instruction::{AccountMeta, Instruction},
};

use crate::constants::{
    PUMP_BUY_ATA_PROGRAM, PUMP_BUY_BASE_MINT, PUMP_BUY_BASE_TOKEN_PROGRAM,
    PUMP_BUY_COIN_CREATOR_VAULT_ATA, PUMP_BUY_COIN_CREATOR_VAULT_AUTH, PUMP_BUY_DISC,
    PUMP_BUY_EVENT_AUTHORITY, PUMP_BUY_FEE_CONFIG, PUMP_BUY_FEE_PROGRAM, PUMP_BUY_FIXED_LEN,
    PUMP_BUY_GLOBAL_CONFIG, PUMP_BUY_GLOBAL_VOL_ACCUM, PUMP_BUY_POOL, PUMP_BUY_POOL_BASE_ATA,
    PUMP_BUY_POOL_QUOTE_ATA, PUMP_BUY_PROGRAM, PUMP_BUY_PROTOCOL_FEE_ATA,
    PUMP_BUY_PROTOCOL_FEE_RECIPIENT, PUMP_BUY_QUOTE_MINT, PUMP_BUY_QUOTE_TOKEN_PROGRAM,
    PUMP_BUY_SYSTEM_PROGRAM, PUMP_BUY_USER, PUMP_BUY_USER_BASE_ATA, PUMP_BUY_USER_QUOTE_ATA,
    PUMP_BUY_USER_VOL_ACCUM, PUMP_SELL_ATA_PROGRAM, PUMP_SELL_BASE_MINT,
    PUMP_SELL_BASE_TOKEN_PROGRAM, PUMP_SELL_COIN_CREATOR_VAULT_ATA,
    PUMP_SELL_COIN_CREATOR_VAULT_AUTH, PUMP_SELL_DISC, PUMP_SELL_EVENT_AUTHORITY,
    PUMP_SELL_FEE_CONFIG, PUMP_SELL_FEE_PROGRAM, PUMP_SELL_FIXED_LEN, PUMP_SELL_GLOBAL_CONFIG,
    PUMP_SELL_POOL, PUMP_SELL_POOL_BASE_ATA, PUMP_SELL_POOL_QUOTE_ATA, PUMP_SELL_PROGRAM,
    PUMP_SELL_PROTOCOL_FEE_ATA, PUMP_SELL_PROTOCOL_FEE_RECIPIENT, PUMP_SELL_QUOTE_MINT,
    PUMP_SELL_QUOTE_TOKEN_PROGRAM, PUMP_SELL_SYSTEM_PROGRAM, PUMP_SELL_USER,
    PUMP_SELL_USER_BASE_ATA, PUMP_SELL_USER_QUOTE_ATA,
};

// ── Helpers ──────────────────────────────────────────────────────────

/// Build an `AccountMeta` vector by walking a list of (rel_offset, writable) pairs.
/// `pump_base` is the absolute index where the PumpSwap section starts.
fn build_meta(
    accounts: &[AccountInfo],
    pump_base: usize,
    slots: &[(usize, bool)],
) -> Vec<AccountMeta> {
    slots
        .iter()
        .map(|&(rel, writable)| {
            let a = &accounts[pump_base + rel];
            if writable {
                AccountMeta::new(*a.key, a.is_signer)
            } else {
                AccountMeta::new_readonly(*a.key, a.is_signer)
            }
        })
        .collect()
}

// ── buy_exact_quote_in ───────────────────────────────────────────────

/// Build a PumpSwap `buy_exact_quote_in` CPI instruction.
///
/// Spends `amount_in` lamports of quote (WSOL), receiving at least
/// `min_amount_out` base tokens (meme). `track_volume` is forwarded
/// to the PumpSwap instruction as-is.
///
/// `pump_remaining_count` must match the number of remaining accounts
/// placed after the fixed 23. Each remaining account's writable flag is
/// read from `AccountInfo::is_writable` — the client MUST set those flags
/// correctly in the transaction's AccountMeta.
pub fn build_buy(
    accounts: &[AccountInfo],
    pump_base: usize,
    pump_remaining_count: usize,
    amount_in: u64,
    min_amount_out: u64,
    track_volume: bool,
    user_base_ata_override: Option<usize>,
) -> Instruction {
    // Fixed account slots in buy_exact_quote_in IDL order.
    let slots: &[(usize, bool)] = &[
        (PUMP_BUY_POOL, true),
        (PUMP_BUY_USER, true),
        (PUMP_BUY_GLOBAL_CONFIG, false),
        (PUMP_BUY_BASE_MINT, false),
        (PUMP_BUY_QUOTE_MINT, false),
        (PUMP_BUY_USER_BASE_ATA, true),
        (PUMP_BUY_USER_QUOTE_ATA, true),
        (PUMP_BUY_POOL_BASE_ATA, true),
        (PUMP_BUY_POOL_QUOTE_ATA, true),
        (PUMP_BUY_PROTOCOL_FEE_RECIPIENT, false),
        (PUMP_BUY_PROTOCOL_FEE_ATA, true),
        (PUMP_BUY_BASE_TOKEN_PROGRAM, false),
        (PUMP_BUY_QUOTE_TOKEN_PROGRAM, false),
        (PUMP_BUY_SYSTEM_PROGRAM, false),
        (PUMP_BUY_ATA_PROGRAM, false),
        (PUMP_BUY_EVENT_AUTHORITY, false),
        (PUMP_BUY_PROGRAM, false),
        (PUMP_BUY_COIN_CREATOR_VAULT_ATA, true),
        (PUMP_BUY_COIN_CREATOR_VAULT_AUTH, false),
        (PUMP_BUY_GLOBAL_VOL_ACCUM, false),
        (PUMP_BUY_USER_VOL_ACCUM, true),
        (PUMP_BUY_FEE_CONFIG, false),
        (PUMP_BUY_FEE_PROGRAM, false),
    ];

    let mut meta = build_meta(accounts, pump_base, slots);
    if let Some(idx) = user_base_ata_override {
        meta[5] = AccountMeta::new(*accounts[idx].key, accounts[idx].is_signer);
    }

    // Remaining accounts: pass through, preserving the client-set writable flag.
    // Order: [cashback_ata?] [pool_v2_pda?] [buyback_recipient] [buyback_ata]
    for i in 0..pump_remaining_count {
        let a = &accounts[pump_base + PUMP_BUY_FIXED_LEN + i];
        if a.is_writable {
            meta.push(AccountMeta::new(*a.key, a.is_signer));
        } else {
            meta.push(AccountMeta::new_readonly(*a.key, a.is_signer));
        }
    }

    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&PUMP_BUY_DISC);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());
    data.push(track_volume as u8);

    Instruction {
        program_id: *accounts[pump_base + PUMP_BUY_PROGRAM].key,
        accounts: meta,
        data,
    }
}

// ── sell ─────────────────────────────────────────────────────────────

/// Build a PumpSwap `sell` CPI instruction.
///
/// Sells `amount_in` base tokens (meme), receiving at least
/// `min_amount_out` quote tokens (WSOL lamports).
///
/// `pump_remaining_count` must match the number of remaining accounts
/// placed after the fixed 21. Each remaining account's writable flag is
/// read from `AccountInfo::is_writable` — the client MUST set those flags
/// correctly in the transaction's AccountMeta.
pub fn build_sell(
    accounts: &[AccountInfo],
    pump_base: usize,
    pump_remaining_count: usize,
    amount_in: u64,
    min_amount_out: u64,
    user_base_ata_override: Option<usize>,
) -> Instruction {
    // Fixed account slots in sell IDL order.
    // Sell omits global_vol_accum and user_vol_accum from the fixed list.
    let slots: &[(usize, bool)] = &[
        (PUMP_SELL_POOL, true),
        (PUMP_SELL_USER, true),
        (PUMP_SELL_GLOBAL_CONFIG, false),
        (PUMP_SELL_BASE_MINT, false),
        (PUMP_SELL_QUOTE_MINT, false),
        (PUMP_SELL_USER_BASE_ATA, true),
        (PUMP_SELL_USER_QUOTE_ATA, true),
        (PUMP_SELL_POOL_BASE_ATA, true),
        (PUMP_SELL_POOL_QUOTE_ATA, true),
        (PUMP_SELL_PROTOCOL_FEE_RECIPIENT, false),
        (PUMP_SELL_PROTOCOL_FEE_ATA, true),
        (PUMP_SELL_BASE_TOKEN_PROGRAM, false),
        (PUMP_SELL_QUOTE_TOKEN_PROGRAM, false),
        (PUMP_SELL_SYSTEM_PROGRAM, false),
        (PUMP_SELL_ATA_PROGRAM, false),
        (PUMP_SELL_EVENT_AUTHORITY, false),
        (PUMP_SELL_PROGRAM, false),
        (PUMP_SELL_COIN_CREATOR_VAULT_ATA, true),
        (PUMP_SELL_COIN_CREATOR_VAULT_AUTH, false),
        (PUMP_SELL_FEE_CONFIG, false),
        (PUMP_SELL_FEE_PROGRAM, false),
    ];

    let mut meta = build_meta(accounts, pump_base, slots);
    if let Some(idx) = user_base_ata_override {
        meta[5] = AccountMeta::new(*accounts[idx].key, accounts[idx].is_signer);
    }

    // Remaining accounts: pass through, preserving the client-set writable flag.
    // Order: [cashback_ata?] [user_vol_accum?] [pool_v2_pda?] [buyback_recipient] [buyback_ata]
    for i in 0..pump_remaining_count {
        let a = &accounts[pump_base + PUMP_SELL_FIXED_LEN + i];
        if a.is_writable {
            meta.push(AccountMeta::new(*a.key, a.is_signer));
        } else {
            meta.push(AccountMeta::new_readonly(*a.key, a.is_signer));
        }
    }

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMP_SELL_DISC);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());

    Instruction {
        program_id: *accounts[pump_base + PUMP_SELL_PROGRAM].key,
        accounts: meta,
        data,
    }
}
