//! Raydium CPMM instruction builder
#![allow(dead_code)] // reserved for venue expansion

use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::{
    CPMM_SWAP_DISCRIMINATOR, MEMO_PROGRAM, NATIVE_SOL_MINT, TOKEN22_PROGRAM, TOKEN_PROGRAM,
};
use crate::pool_cache::PoolStateData;

/// Determine which side of the pool is SOL (or stablecoin) and which side is the meme coin
pub fn resolve_pool_sides(
    pool: &PoolStateData,
) -> Option<(Pubkey, Pubkey, String, String, u64, u64)> {
    let t0_sol = pool.token_0_mint == NATIVE_SOL_MINT;
    let t1_sol = pool.token_1_mint == NATIVE_SOL_MINT;

    let (sol_mint, meme_mint, sol_vault, meme_vault, sol_raw, meme_raw) = if t0_sol {
        (
            super::pubkey_from_str(&pool.token_0_mint)?,
            super::pubkey_from_str(&pool.token_1_mint)?,
            pool.token_0_vault.clone(),
            pool.token_1_vault.clone(),
            pool.token_0_vault_raw,
            pool.token_1_vault_raw,
        )
    } else if t1_sol {
        (
            super::pubkey_from_str(&pool.token_1_mint)?,
            super::pubkey_from_str(&pool.token_0_mint)?,
            pool.token_1_vault.clone(),
            pool.token_0_vault.clone(),
            pool.token_1_vault_raw,
            pool.token_0_vault_raw,
        )
    } else {
        return None;
    };

    Some((
        sol_mint, meme_mint, sol_vault, meme_vault, sol_raw, meme_raw,
    ))
}

/// Build CPMM swap instruction
#[allow(clippy::too_many_arguments)]
pub fn build_cpmm_swap_ix(
    payer: &Pubkey,
    pool_state: &Pubkey,
    amm_config: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
    input_ata: &Pubkey,
    output_ata: &Pubkey,
    input_vault: &Pubkey,
    output_vault: &Pubkey,
    amount_in: u64,
    min_amount_out: u64,
    cpmm_program: &Pubkey,
) -> Instruction {
    let authority =
        Pubkey::find_program_address(&[b"vault_and_lp", &pool_state.to_bytes()], cpmm_program).0;

    let token_program = Pubkey::from_str(TOKEN_PROGRAM).unwrap();
    let memo_program = Pubkey::from_str(MEMO_PROGRAM).unwrap();
    let token22_program = Pubkey::from_str(TOKEN22_PROGRAM).unwrap();

    let accounts = vec![
        AccountMeta::new(*payer, true),
        AccountMeta::new_readonly(authority, false),
        AccountMeta::new_readonly(*amm_config, false),
        AccountMeta::new(*pool_state, false),
        AccountMeta::new(*input_ata, false),
        AccountMeta::new(*output_ata, false),
        AccountMeta::new(*input_vault, false),
        AccountMeta::new(*output_vault, false),
        AccountMeta::new_readonly(*input_mint, false),
        AccountMeta::new_readonly(*output_mint, false),
        AccountMeta::new_readonly(token_program, false),
        AccountMeta::new_readonly(memo_program, false),
        AccountMeta::new_readonly(token22_program, false),
    ];

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&CPMM_SWAP_DISCRIMINATOR);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());

    Instruction {
        program_id: *cpmm_program,
        accounts,
        data,
    }
}
