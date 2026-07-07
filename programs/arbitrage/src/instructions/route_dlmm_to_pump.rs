//! Route: DLMM swap2 (SOL → meme) → PumpSwap sell (meme → SOL).
//!
//! Account layout (mirrored from route_pump_to_dlmm, DLMM first):
//!   [0..=2]    shared user accounts
//!   [3..=11]   DLMM fixed (9) + bin arrays (1..=4)
//!   [12..]     PumpSwap Sell fixed (21) + remaining (0..=5)

use alloc::format;
use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult, msg,
    program::invoke, program_error::ProgramError, pubkey::Pubkey,
};

use crate::{
    accounting,
    constants::*,
    cpi::{dlmm, pump_swap},
    error::{
        arb_err, ARB_BAD_ACCOUNT_COUNT, ARB_BAD_MINT, ARB_BAD_PDA, ARB_BAD_PROGRAM,
        ARB_INSUFFICIENT_PROFIT, ARB_NEGATIVE_NET, ARB_RESIDUAL_MEME, ARB_ZERO_AMOUNT,
    },
};

pub fn handle(accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    // 1. Parse instruction data (same 36-byte layout) ───────────────
    if data.len() != IX_DATA_LEN {
        return Err(ProgramError::InvalidInstructionData);
    }

    let amount_in = u64::from_le_bytes(data[OFF_AMOUNT_IN..OFF_AMOUNT_IN + 8].try_into().unwrap());
    let min_profit_lamports =
        u64::from_le_bytes(data[OFF_MIN_PROFIT..OFF_MIN_PROFIT + 8].try_into().unwrap());
    let min_intermediate_meme = u64::from_le_bytes(
        data[OFF_MIN_INTERMEDIATE..OFF_MIN_INTERMEDIATE + 8]
            .try_into()
            .unwrap(),
    );
    let _track_volume = data[OFF_TRACK_VOLUME] != 0;
    let dlmm_sol_is_x = data[OFF_DLMM_SOL_IS_X] != 0;
    let pump_remaining_count = data[OFF_PUMP_REMAINING] as usize;
    let dlmm_bin_array_count = data[OFF_DLMM_BIN_ARRAY_COUNT] as usize;

    // 2. Pre-CPI checks ─────────────────────────────────────────────
    if amount_in == 0 || min_profit_lamports == 0 {
        return Err(arb_err(ARB_ZERO_AMOUNT));
    }
    if pump_remaining_count > 5 || !(1..=6).contains(&dlmm_bin_array_count) {
        return Err(arb_err(ARB_BAD_ACCOUNT_COUNT));
    }

    let dlmm_base = SHARED_FIXED_LEN;
    let pump_sell_base = dlmm_base + DLMM_FIXED_LEN + dlmm_bin_array_count;

    let total_expected = pump_sell_base + PUMP_SELL_FIXED_LEN + pump_remaining_count;
    if accounts.len() < total_expected {
        return Err(arb_err(ARB_BAD_ACCOUNT_COUNT));
    }

    // Program ID checks
    if accounts[dlmm_base + DLMM_PROGRAM_REL].key != &DLMM_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }
    if accounts[pump_sell_base + PUMP_SELL_PROGRAM].key != &PUMP_SWAP_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // Well-known accounts in PumpSwap Sell section
    if accounts[pump_sell_base + PUMP_SELL_QUOTE_MINT].key != &NATIVE_SOL_MINT {
        return Err(arb_err(ARB_BAD_MINT));
    }
    if accounts[pump_sell_base + PUMP_SELL_QUOTE_TOKEN_PROGRAM].key != &TOKEN_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }
    if accounts[pump_sell_base + PUMP_SELL_SYSTEM_PROGRAM].key != &SYSTEM_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }
    if accounts[pump_sell_base + PUMP_SELL_ATA_PROGRAM].key != &ATA_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }
    if accounts[pump_sell_base + PUMP_SELL_FEE_PROGRAM].key != &FEE_PROGRAM_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // Signer check
    if !accounts[USER_IDX].is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // DLMM well-known accounts
    if accounts[dlmm_base + DLMM_EVENT_AUTH_REL].key != &DLMM_EVENT_AUTH {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }
    if accounts[dlmm_base + DLMM_MEMO_REL].key != &MEMO_ID {
        return Err(arb_err(ARB_BAD_PROGRAM));
    }

    // PDA: DLMM oracle
    {
        let lb_pair = accounts[dlmm_base + DLMM_LB_PAIR_REL].key;
        let (expected, _) =
            Pubkey::find_program_address(&[DLMM_ORACLE_SEED, lb_pair.as_ref()], &DLMM_ID);
        if accounts[dlmm_base + DLMM_ORACLE_REL].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // PDA: PumpSwap event authority
    {
        let (expected, _) = Pubkey::find_program_address(&[PUMP_EVENT_AUTH_SEED], &PUMP_SWAP_ID);
        if accounts[pump_sell_base + PUMP_SELL_EVENT_AUTHORITY].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // PDA: PumpSwap global config
    {
        let (expected, _) = Pubkey::find_program_address(&[PUMP_GLOBAL_CONFIG_SEED], &PUMP_SWAP_ID);
        if accounts[pump_sell_base + PUMP_SELL_GLOBAL_CONFIG].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // PDA: fee config
    {
        let (expected, _) = Pubkey::find_program_address(
            &[PUMP_FEE_CONFIG_SEED, PUMP_SWAP_ID.as_ref()],
            &FEE_PROGRAM_ID,
        );
        if accounts[pump_sell_base + PUMP_SELL_FEE_CONFIG].key != &expected {
            return Err(arb_err(ARB_BAD_PDA));
        }
    }

    // 3. TP/ATA selection ──────────────────────────────────────────────
    // Client resolves the correct token program via RPC (mint owner) and
    // places it at PUMP_SELL_BASE_TOKEN_PROGRAM. The primary ATA at
    // USER_MEME_ATA_IDX is always created in the TX with the correct
    // token program. Do NOT switch to alt accounts.
    let meme_tp_idx = pump_sell_base + PUMP_SELL_BASE_TOKEN_PROGRAM;
    let meme_ata_idx = USER_MEME_ATA_IDX;

    // 4. Balance snapshots ──────────────────────────────────────────
    let pre_aggregate =
        accounting::aggregate_sol_balance(&accounts[USER_IDX], &accounts[USER_SOL_ATA_IDX])?;
    let meme_before = accounting::read_token_amount(&accounts[meme_ata_idx])?;

    // 5. CPI: DLMM swap2 (buy meme with SOL) ────────────────────────
    let (token_x_prog, token_y_prog) = if dlmm_sol_is_x {
        (pump_sell_base + PUMP_SELL_QUOTE_TOKEN_PROGRAM, meme_tp_idx)
    } else {
        (meme_tp_idx, pump_sell_base + PUMP_SELL_QUOTE_TOKEN_PROGRAM)
    };
    let (token_x_mint, token_y_mint) = if dlmm_sol_is_x {
        (pump_sell_base + PUMP_SELL_QUOTE_MINT, pump_sell_base + PUMP_SELL_BASE_MINT)
    } else {
        (pump_sell_base + PUMP_SELL_BASE_MINT, pump_sell_base + PUMP_SELL_QUOTE_MINT)
    };

    let swap2_ix = dlmm::build_swap2(
        accounts,
        dlmm_base,
        amount_in,
        token_x_mint,
        token_y_mint,
        token_x_prog,
        token_y_prog,
        USER_IDX,
        USER_SOL_ATA_IDX, // user_token_in = WSOL
        meme_ata_idx,     // user_token_out = meme
        dlmm_bin_array_count,
    );
    invoke(&swap2_ix, accounts)
        .map_err(|e| { msg!("DLMM buy CPI failed"); arb_err(crate::error::ARB_DLMM_CPI_FAILED) })?;

    // 6. CPI: PumpSwap sell (sell all received meme for SOL) ────────
    let meme_after_buy = accounting::read_token_amount(&accounts[meme_ata_idx])?;
    let meme_received = meme_after_buy
        .checked_sub(meme_before)
        .ok_or(arb_err(ARB_NEGATIVE_NET))?;
    if meme_received == 0 {
        return Err(arb_err(ARB_ZERO_AMOUNT));
    }

    let sell_ix = pump_swap::build_sell(
        accounts,
        pump_sell_base,
        pump_remaining_count,
        meme_received,
        min_intermediate_meme,
        if meme_ata_idx == USER_MEME_ATA_IDX { None } else { Some(meme_ata_idx) },
    );
    invoke(&sell_ix, accounts)
        .map_err(|e| { msg!("PumpSwap sell CPI failed"); arb_err(crate::error::ARB_PUMP_CPI_FAILED) })?;

    // 7. Post-CPI invariants ────────────────────────────────────────
    let post_aggregate =
        accounting::aggregate_sol_balance(&accounts[USER_IDX], &accounts[USER_SOL_ATA_IDX])?;

    let net_sol = post_aggregate
        .checked_sub(pre_aggregate)
        .ok_or(arb_err(ARB_NEGATIVE_NET))?;
    if net_sol < min_profit_lamports {
        return Err(arb_err(ARB_INSUFFICIENT_PROFIT));
    }

    let meme_after = accounting::read_token_amount(&accounts[meme_ata_idx])?;
    if meme_after != meme_before {
        return Err(arb_err(ARB_RESIDUAL_MEME));
    }

    Ok(())
}
