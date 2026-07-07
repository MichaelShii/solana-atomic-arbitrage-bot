//! PumpSwap AMM instruction builders (graduated tokens on pAMMBay6oceH...)
//!
//! Supports buy_exact_quote_in (spend exact SOL → get at least X tokens) and
//! sell (spend exact tokens → get at least X SOL).
//!
//! Reference: official pump-swap-sdk IDL (crate::instruction::swap_accounts).

use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::{
    checked_estimate_swap_output, estimate_swap_output, ATA_PROGRAM, PUMPSWAP_BUY_DISCRIMINATOR,
    PUMPSWAP_SELL_DISCRIMINATOR,
};
use crate::constants::{
    PUMPFUN_AMM_PROGRAM, PUMPSWAP_EVENT_AUTHORITY, PUMPSWAP_FEE_PROGRAM, PUMPSWAP_GLOBAL_CONFIG,
    PUMPSWAP_GLOBAL_VOLUME_ACCUMULATOR,
};

mod math;
pub use math::*;
mod state;
pub use state::*;

// ============================================================
// PDA helpers
// ============================================================

/// PDA: ["user_volume_accumulator", user] under pump-amm
pub fn pumpswap_user_vol_accumulator(user: &Pubkey) -> Pubkey {
    let program = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"user_volume_accumulator", &user.to_bytes()], &program).0
}

/// PDA: ["creator_vault", coin_creator] under pump-amm
pub fn pumpswap_coin_creator_vault_authority(coin_creator: &Pubkey) -> Pubkey {
    let program = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"creator_vault", &coin_creator.to_bytes()], &program).0
}

/// ATA for (authority, quote_mint) under the given token program
pub fn pumpswap_coin_creator_vault_ata(
    authority: &Pubkey,
    quote_mint: &Pubkey,
    quote_token_program: &Pubkey,
) -> Pubkey {
    super::ata_addr(authority, quote_mint, quote_token_program)
}

/// PDA: ["fee_config", pump_amm] under fee_program
pub fn pumpswap_fee_config_pda() -> Pubkey {
    let fee_program = Pubkey::from_str(PUMPSWAP_FEE_PROGRAM).unwrap();
    let pump_amm = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"fee_config", &pump_amm.to_bytes()], &fee_program).0
}

/// PDA: ["pool-v2", base_mint] under pump-amm
/// NOTE: This derivation is WRONG and only kept for tests.
/// The correct derivation uses ["pool", 0u16::LE, creator, base_mint, SOL_mint]
/// where creator is the wallet that called migrate (read from bonding curve offset 49).
pub fn pumpswap_pool_v2_pda(base_mint: &Pubkey) -> Pubkey {
    let program = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"pool-v2", &base_mint.to_bytes()], &program).0
}

/// Correct Pool PDA: ["pool", 0u16::LE, creator, base_mint, SOL_mint] @ pAMMBay6oceH
pub fn pumpswap_pool_pda(creator: &Pubkey, base_mint: &Pubkey) -> Pubkey {
    let program = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();
    let sol_mint = Pubkey::from_str(super::NATIVE_SOL_MINT).unwrap();
    Pubkey::find_program_address(
        &[
            b"pool",
            &0u16.to_le_bytes(),
            &creator.to_bytes(),
            &base_mint.to_bytes(),
            &sol_mint.to_bytes(),
        ],
        &program,
    )
    .0
}

/// ATA owned by user_volume_accumulator(user) for the given quote mint
pub fn pumpswap_user_vol_accumulator_quote_ata(
    user: &Pubkey,
    quote_mint: &Pubkey,
    quote_token_program: &Pubkey,
) -> Pubkey {
    let vol_accum = pumpswap_user_vol_accumulator(user);
    super::ata_addr(&vol_accum, quote_mint, quote_token_program)
}

// ============================================================
// Instruction builders
// ============================================================

/// Build a PumpSwap AMM `buy_exact_quote_in` instruction.
///
/// Spends exactly `spendable_quote_in` lamports of quote (WSOL), receiving at
/// least `min_base_amount_out` base tokens (meme).
///
/// `buyback_recipient` is one of the 8 well-known buyback fee recipient
/// addresses (randomly selected).
/// `protocol_fee_recipient` is one of the 8 protocol fee recipients (or 8
/// reserved recipients for Mayhem-mode pools), picked based on
/// `Pool.is_mayhem_mode`.
#[allow(clippy::too_many_arguments)]
pub fn build_pumpswap_buy_ix(
    user: &Pubkey,
    pool: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    user_base_ata: &Pubkey,
    user_quote_ata: &Pubkey,
    pool_base_ata: &Pubkey,
    pool_quote_ata: &Pubkey,
    base_token_program: &Pubkey,
    quote_token_program: &Pubkey,
    spendable_quote_in: u64,
    min_base_amount_out: u64,
    track_volume: bool,
    coin_creator: &Pubkey,
    is_cashback_coin: bool,
    buyback_recipient: &Pubkey,
    protocol_fee_recipient: &Pubkey,
) -> Instruction {
    let pump_program = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let ata_program = Pubkey::from_str(ATA_PROGRAM).unwrap();
    let global_config = Pubkey::from_str(PUMPSWAP_GLOBAL_CONFIG).unwrap();
    let event_authority = Pubkey::from_str(PUMPSWAP_EVENT_AUTHORITY).unwrap();
    let global_vol_accum = Pubkey::from_str(PUMPSWAP_GLOBAL_VOLUME_ACCUMULATOR).unwrap();
    let fee_program = Pubkey::from_str(PUMPSWAP_FEE_PROGRAM).unwrap();

    let protocol_fee_recipient_ata =
        super::ata_addr(protocol_fee_recipient, quote_mint, quote_token_program);

    let creator_vault_authority = pumpswap_coin_creator_vault_authority(coin_creator);
    let creator_vault_ata =
        pumpswap_coin_creator_vault_ata(&creator_vault_authority, quote_mint, quote_token_program);

    let fee_config = pumpswap_fee_config_pda();
    let user_vol_accum = pumpswap_user_vol_accumulator(user);

    let mut accounts = vec![
        AccountMeta::new(*pool, false),
        AccountMeta::new(*user, true),
        AccountMeta::new_readonly(global_config, false),
        AccountMeta::new_readonly(*base_mint, false),
        AccountMeta::new_readonly(*quote_mint, false),
        AccountMeta::new(*user_base_ata, false),
        AccountMeta::new(*user_quote_ata, false),
        AccountMeta::new(*pool_base_ata, false),
        AccountMeta::new(*pool_quote_ata, false),
        AccountMeta::new_readonly(*protocol_fee_recipient, false),
        AccountMeta::new(protocol_fee_recipient_ata, false),
        AccountMeta::new_readonly(*base_token_program, false),
        AccountMeta::new_readonly(*quote_token_program, false),
        AccountMeta::new_readonly(system_program, false),
        AccountMeta::new_readonly(ata_program, false),
        AccountMeta::new_readonly(event_authority, false),
        AccountMeta::new_readonly(pump_program, false),
        AccountMeta::new(creator_vault_ata, false),
        AccountMeta::new_readonly(creator_vault_authority, false),
        AccountMeta::new_readonly(global_vol_accum, false),
        AccountMeta::new(user_vol_accum, false),
        AccountMeta::new_readonly(fee_config, false),
        AccountMeta::new_readonly(fee_program, false),
    ];

    // remaining accounts (mirrors official SDK append_swap_remaining_accounts)
    if is_cashback_coin {
        let cashback_ata =
            pumpswap_user_vol_accumulator_quote_ata(user, quote_mint, quote_token_program);
        accounts.push(AccountMeta::new(cashback_ata, false));
    }
    if *coin_creator != Pubkey::default() {
        accounts.push(AccountMeta::new_readonly(
            pumpswap_pool_v2_pda(base_mint),
            false,
        ));
    }
    let buyback_recipient_ata = super::ata_addr(buyback_recipient, quote_mint, quote_token_program);
    accounts.push(AccountMeta::new_readonly(*buyback_recipient, false));
    accounts.push(AccountMeta::new(buyback_recipient_ata, false));

    let mut data = Vec::with_capacity(25);
    data.extend_from_slice(&PUMPSWAP_BUY_DISCRIMINATOR);
    data.extend_from_slice(&spendable_quote_in.to_le_bytes());
    data.extend_from_slice(&min_base_amount_out.to_le_bytes());
    data.push(track_volume as u8);

    Instruction {
        program_id: pump_program,
        accounts,
        data,
    }
}

/// Build a PumpSwap AMM `sell` instruction.
///
/// Sells exactly `base_amount_in` base tokens (meme), receiving at least
/// `min_quote_amount_out` quote tokens (WSOL lamports).
///
/// `buyback_recipient` is one of the 8 well-known buyback fee recipient
/// addresses (randomly selected).
/// `protocol_fee_recipient` is one of the 8 protocol fee recipients (or 8
/// reserved recipients for Mayhem-mode pools), picked based on
/// `Pool.is_mayhem_mode`.
#[allow(clippy::too_many_arguments)]
pub fn build_pumpswap_sell_ix(
    user: &Pubkey,
    pool: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    user_base_ata: &Pubkey,
    user_quote_ata: &Pubkey,
    pool_base_ata: &Pubkey,
    pool_quote_ata: &Pubkey,
    base_token_program: &Pubkey,
    quote_token_program: &Pubkey,
    base_amount_in: u64,
    min_quote_amount_out: u64,
    coin_creator: &Pubkey,
    is_cashback_coin: bool,
    buyback_recipient: &Pubkey,
    protocol_fee_recipient: &Pubkey,
) -> Instruction {
    let pump_program = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let ata_program = Pubkey::from_str(ATA_PROGRAM).unwrap();
    let global_config = Pubkey::from_str(PUMPSWAP_GLOBAL_CONFIG).unwrap();
    let event_authority = Pubkey::from_str(PUMPSWAP_EVENT_AUTHORITY).unwrap();
    let fee_program = Pubkey::from_str(PUMPSWAP_FEE_PROGRAM).unwrap();

    let protocol_fee_recipient_ata =
        super::ata_addr(protocol_fee_recipient, quote_mint, quote_token_program);

    let creator_vault_authority = pumpswap_coin_creator_vault_authority(coin_creator);
    let creator_vault_ata =
        pumpswap_coin_creator_vault_ata(&creator_vault_authority, quote_mint, quote_token_program);

    let fee_config = pumpswap_fee_config_pda();

    // Sell omits global/user volume accumulators from the fixed list (they
    // only appear in remaining_accounts when is_cashback_coin).
    let mut accounts = vec![
        AccountMeta::new(*pool, false),
        AccountMeta::new(*user, true),
        AccountMeta::new_readonly(global_config, false),
        AccountMeta::new_readonly(*base_mint, false),
        AccountMeta::new_readonly(*quote_mint, false),
        AccountMeta::new(*user_base_ata, false),
        AccountMeta::new(*user_quote_ata, false),
        AccountMeta::new(*pool_base_ata, false),
        AccountMeta::new(*pool_quote_ata, false),
        AccountMeta::new_readonly(*protocol_fee_recipient, false),
        AccountMeta::new(protocol_fee_recipient_ata, false),
        AccountMeta::new_readonly(*base_token_program, false),
        AccountMeta::new_readonly(*quote_token_program, false),
        AccountMeta::new_readonly(system_program, false),
        AccountMeta::new_readonly(ata_program, false),
        AccountMeta::new_readonly(event_authority, false),
        AccountMeta::new_readonly(pump_program, false),
        AccountMeta::new(creator_vault_ata, false),
        AccountMeta::new_readonly(creator_vault_authority, false),
        AccountMeta::new_readonly(fee_config, false),
        AccountMeta::new_readonly(fee_program, false),
    ];

    // remaining accounts: Sell adds user_volume_accumulator (writable) when cashback
    if is_cashback_coin {
        let cashback_ata =
            pumpswap_user_vol_accumulator_quote_ata(user, quote_mint, quote_token_program);
        accounts.push(AccountMeta::new(cashback_ata, false));
        accounts.push(AccountMeta::new(pumpswap_user_vol_accumulator(user), false));
    }
    if *coin_creator != Pubkey::default() {
        accounts.push(AccountMeta::new_readonly(
            pumpswap_pool_v2_pda(base_mint),
            false,
        ));
    }
    let buyback_recipient_ata = super::ata_addr(buyback_recipient, quote_mint, quote_token_program);
    accounts.push(AccountMeta::new_readonly(*buyback_recipient, false));
    accounts.push(AccountMeta::new(buyback_recipient_ata, false));

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMPSWAP_SELL_DISCRIMINATOR);
    data.extend_from_slice(&base_amount_in.to_le_bytes());
    data.extend_from_slice(&min_quote_amount_out.to_le_bytes());

    Instruction {
        program_id: pump_program,
        accounts,
        data,
    }
}

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod unit_tests;
