//! Pump.fun bonding curve instruction builder (pre-graduation, program 6EF8rrecth...)

use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::{
    ATA_PROGRAM, PUMPFUN_BONDING_CURVE_PROGRAM, PUMPFUN_BUY_DISCRIMINATOR,
    PUMPFUN_SELL_DISCRIMINATOR, SYSVAR_RENT,
};

/// Pump.fun global PDA (seed = "global")
pub fn pumpfun_global_pda() -> Pubkey {
    let program = Pubkey::from_str(PUMPFUN_BONDING_CURVE_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"global"], &program).0
}

/// Pump.fun event authority PDA
pub fn pumpfun_event_authority_pda() -> Pubkey {
    let program = Pubkey::from_str(PUMPFUN_BONDING_CURVE_PROGRAM).unwrap();
    Pubkey::find_program_address(&[b"event-authority"], &program).0
}

/// Build Pump.fun BuyExactQuoteIn instruction
#[allow(clippy::too_many_arguments)]
pub fn build_pumpfun_buy_ix(
    user: &Pubkey,
    mint: &Pubkey,
    bonding_curve: &Pubkey,
    associated_bonding_curve: &Pubkey,
    associated_user: &Pubkey,
    fee_recipient: &Pubkey,
    amount_lamports: u64,
    min_amount_out: u64,
    token_program: &Pubkey,
) -> Instruction {
    let pumpfun = Pubkey::from_str(PUMPFUN_BONDING_CURVE_PROGRAM).unwrap();
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let ata_program = Pubkey::from_str(ATA_PROGRAM).unwrap();
    let rent = Pubkey::from_str(SYSVAR_RENT).unwrap();
    let global = pumpfun_global_pda();
    let event_authority = pumpfun_event_authority_pda();

    let accounts = vec![
        AccountMeta::new_readonly(global, false),
        AccountMeta::new(*user, true),
        AccountMeta::new_readonly(*mint, false),
        AccountMeta::new(*bonding_curve, false),
        AccountMeta::new(*associated_bonding_curve, false),
        AccountMeta::new(*associated_user, false),
        AccountMeta::new(*fee_recipient, false),
        AccountMeta::new_readonly(system_program, false),
        AccountMeta::new_readonly(*token_program, false),
        AccountMeta::new_readonly(ata_program, false),
        AccountMeta::new_readonly(rent, false),
        AccountMeta::new_readonly(event_authority, false),
        AccountMeta::new_readonly(pumpfun, false),
    ];

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMPFUN_BUY_DISCRIMINATOR);
    data.extend_from_slice(&amount_lamports.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());

    Instruction {
        program_id: pumpfun,
        accounts,
        data,
    }
}

/// Build Pump.fun Sell instruction
#[allow(clippy::too_many_arguments)]
pub fn build_pumpfun_sell_ix(
    user: &Pubkey,
    mint: &Pubkey,
    bonding_curve: &Pubkey,
    associated_bonding_curve: &Pubkey,
    associated_user: &Pubkey,
    fee_recipient: &Pubkey,
    amount_tokens: u64,
    min_sol_out: u64,
    token_program: &Pubkey,
) -> Instruction {
    let pumpfun = Pubkey::from_str(PUMPFUN_BONDING_CURVE_PROGRAM).unwrap();
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let ata_program = Pubkey::from_str(ATA_PROGRAM).unwrap();
    let rent = Pubkey::from_str(SYSVAR_RENT).unwrap();
    let global = pumpfun_global_pda();
    let event_authority = pumpfun_event_authority_pda();

    let accounts = vec![
        AccountMeta::new_readonly(global, false),
        AccountMeta::new(*user, true),
        AccountMeta::new_readonly(*mint, false),
        AccountMeta::new(*bonding_curve, false),
        AccountMeta::new(*associated_bonding_curve, false),
        AccountMeta::new(*associated_user, false),
        AccountMeta::new(*fee_recipient, false),
        AccountMeta::new_readonly(system_program, false),
        AccountMeta::new_readonly(*token_program, false),
        AccountMeta::new_readonly(ata_program, false),
        AccountMeta::new_readonly(rent, false),
        AccountMeta::new_readonly(event_authority, false),
        AccountMeta::new_readonly(pumpfun, false),
    ];

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&PUMPFUN_SELL_DISCRIMINATOR);
    data.extend_from_slice(&amount_tokens.to_le_bytes());
    data.extend_from_slice(&min_sol_out.to_le_bytes());

    Instruction {
        program_id: pumpfun,
        accounts,
        data,
    }
}

/// Estimate Pump.fun bonding curve buy output
pub fn estimate_pumpfun_buy_output(
    amount_in_sol: u64,
    virtual_sol_reserves: u64,
    virtual_token_reserves: u64,
    fee_bps: u32,
) -> u64 {
    super::estimate_swap_output(
        virtual_sol_reserves,
        virtual_token_reserves,
        amount_in_sol,
        fee_bps as f64 / 10000.0,
    )
}

/// Estimate Pump.fun bonding curve sell output (SOL)
pub fn estimate_pumpfun_sell_output(
    amount_in_tokens: u64,
    virtual_sol_reserves: u64,
    virtual_token_reserves: u64,
    fee_bps: u32,
) -> u64 {
    super::estimate_swap_output(
        virtual_token_reserves,
        virtual_sol_reserves,
        amount_in_tokens,
        fee_bps as f64 / 10000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_fee_reduces_buy_output() {
        // 100 SOL reserves, 1M tokens, buy with 1 SOL
        let out_100bps =
            estimate_pumpfun_buy_output(1_000_000_000, 100_000_000_000, 1_000_000_000_000, 100);
        let out_200bps =
            estimate_pumpfun_buy_output(1_000_000_000, 100_000_000_000, 1_000_000_000_000, 200);
        assert!(
            out_200bps < out_100bps,
            "higher fee should reduce buy output"
        );
    }

    #[test]
    fn higher_fee_reduces_sell_output() {
        let out_100bps =
            estimate_pumpfun_sell_output(1_000_000, 100_000_000_000, 1_000_000_000_000, 100);
        let out_200bps =
            estimate_pumpfun_sell_output(1_000_000, 100_000_000_000, 1_000_000_000_000, 200);
        assert!(
            out_200bps < out_100bps,
            "higher fee should reduce sell output"
        );
    }

    #[test]
    fn zero_fee_is_valid() {
        // 0 bps = no fee; output should equal constant-product amount
        let out = estimate_pumpfun_buy_output(1_000_000_000, 100_000_000_000, 1_000_000_000_000, 0);
        assert!(out > 0, "zero fee should still produce output");
    }
}
