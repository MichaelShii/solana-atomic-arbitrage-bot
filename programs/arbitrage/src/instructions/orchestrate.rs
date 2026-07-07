//! Generic 2-leg arbitrage orchestrator.
//!
//! Executes the canonical flow: validate → snapshot → buy CPI → read
//! intermediate → sell CPI → post-invariants. The specific DEX handlers
//! are selected by matching program IDs from the account list.

use alloc::format;
use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult, msg,
    program::invoke, program_error::ProgramError, pubkey::Pubkey,
};

use crate::{
    accounting,
    constants::*,
    error::{arb_err, ARB_BAD_ACCOUNT_COUNT, ARB_INSUFFICIENT_PROFIT,
            ARB_NEGATIVE_NET, ARB_RESIDUAL_MEME, ARB_UNKNOWN_DEX_PAIR, ARB_ZERO_AMOUNT},
    instructions::{dex_cpmm, dex_dlmm, dex_pumpswap, dex_whirlpool},
};

/// DEX kind identifiers — must match client-side constants.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DexKind {
    PumpSwap,
    Dlmm,
    Cpmm,
    Whirlpool,
}

impl DexKind {
    /// Return the fixed account count for this DEX (excluding remaining accounts).
    pub fn fixed_len(self) -> usize {
        match self {
            DexKind::PumpSwap => dex_pumpswap::PUMPSWAP_BUY_FIXED_LEN,  // 23
            DexKind::Dlmm => dex_dlmm::DLMM_FIXED_LEN,  // 9
            DexKind::Cpmm => dex_cpmm::CPMM_FIXED_LEN,  // 13
            DexKind::Whirlpool => dex_whirlpool::WHIRLPOOL_FIXED_LEN, // 12
        }
    }

    /// Match a program ID to a DEX kind.
    pub fn from_program_id(pid: &Pubkey) -> Option<DexKind> {
        if pid == &PUMP_SWAP_ID { return Some(DexKind::PumpSwap); }
        if pid == &DLMM_ID { return Some(DexKind::Dlmm); }
        if pid == &CPMM_ID { return Some(DexKind::Cpmm); }
        if pid == &WHIRLPOOL_ID { return Some(DexKind::Whirlpool); }
        None
    }
}

/// Execute a 2-leg arbitrage route.
///
/// IX data format (36 bytes):
///   [0..8]   ROUTE_DISC
///   [8..16]  amount_in (u64 LE)
///   [16..24] min_profit_lamports (u64 LE)
///   [24..32] min_intermediate (u64 LE)
///   [32]     track_volume (u8)
///   [33]     buy_is_sol (u8) — SOL is input for the buy leg
///   [34]     buy_remaining_count (u8)
///   [35]     sell_remaining_count (u8)
///
/// Account layout:
///   [0..=2]     shared: user (signer), user_sol_ata, user_meme_ata
///   [3..]       buy DEX section (fixed + remaining)
///   [...]       sell DEX section (fixed + remaining)
pub fn handle(accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    // ── 1. Parse IX data ────────────────────────────────────────────
    if data.len() != IX_DATA_LEN {
        return Err(ProgramError::InvalidInstructionData);
    }

    let amount_in = u64::from_le_bytes(data[OFF_AMOUNT_IN..OFF_AMOUNT_IN + 8].try_into().unwrap());
    let min_profit = u64::from_le_bytes(data[OFF_MIN_PROFIT..OFF_MIN_PROFIT + 8].try_into().unwrap());
    let min_intermediate =
        u64::from_le_bytes(data[OFF_MIN_INTERMEDIATE..OFF_MIN_INTERMEDIATE + 8].try_into().unwrap());
    let _track_volume = data[OFF_TRACK_VOLUME] != 0;
    let buy_is_sol = data[OFF_DLMM_SOL_IS_X] != 0; // reuse same offset for generic flag
    let buy_remaining = data[OFF_BUY_REMAINING] as usize;
    let sell_remaining = data[OFF_SELL_REMAINING] as usize;

    if amount_in == 0 || min_profit == 0 {
        return Err(arb_err(ARB_ZERO_AMOUNT));
    }

    // ── 2. Identify DEXes from account list ─────────────────────────
    // CPMM/Whirlpool/DLMM program at position 0; PumpSwap at position 16.
    // Try both offsets to identify the DEX.
    let buy_base = SHARED_FIXED_LEN;
    let buy_kind = identify_dex(accounts, buy_base)
        .ok_or(arb_err(ARB_UNKNOWN_DEX_PAIR))?;

    let buy_dex_len = buy_kind.fixed_len() + buy_remaining;
    let sell_base = buy_base + buy_dex_len;
    let sell_kind = identify_dex(accounts, sell_base)
        .ok_or(arb_err(ARB_UNKNOWN_DEX_PAIR))?;

    // Can't route through the same DEX
    if buy_kind == sell_kind {
        return Err(arb_err(ARB_UNKNOWN_DEX_PAIR));
    }

    // ── 3. Validate account count ───────────────────────────────────
    let total_expected = sell_base + sell_kind.fixed_len() + sell_remaining;
    if accounts.len() < total_expected {
        return Err(arb_err(ARB_BAD_ACCOUNT_COUNT));
    }

    // ── 4. Signer check ─────────────────────────────────────────────
    if !accounts[USER_IDX].is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // ── 5. Validate DEX sections ────────────────────────────────────
    validate_dex_section(accounts, buy_kind, buy_base, buy_remaining)?;
    validate_dex_section(accounts, sell_kind, sell_base, sell_remaining)?;

    // ── 6. Balance snapshots ────────────────────────────────────────
    let pre_aggregate =
        accounting::aggregate_sol_balance(&accounts[USER_IDX], &accounts[USER_SOL_ATA_IDX])?;
    let meme_before = accounting::read_token_amount(&accounts[USER_MEME_ATA_IDX])?;

    // ── 7. CPI: Buy leg ─────────────────────────────────────────────
    // Determine the meme token ATA position: buy leg output goes to
    // USER_MEME_ATA_IDX when buying meme with SOL.
    execute_buy_leg(accounts, buy_kind, buy_base, buy_remaining,
        amount_in, min_intermediate, buy_is_sol)?;

    // ── 8. Read intermediate output ─────────────────────────────────
    let meme_after_buy = accounting::read_token_amount(&accounts[USER_MEME_ATA_IDX])?;
    let meme_received = meme_after_buy
        .checked_sub(meme_before)
        .ok_or(arb_err(ARB_NEGATIVE_NET))?;
    if meme_received == 0 {
        return Err(arb_err(ARB_ZERO_AMOUNT));
    }

    // ── 9. CPI: Sell leg ─────────────────────────────────────────────
    // Sell all received meme for SOL.
    execute_sell_leg(accounts, sell_kind, sell_base, sell_remaining,
        meme_received, 1, buy_is_sol)?;

    // ── 10. Post-CPI invariants ─────────────────────────────────────
    let post_aggregate =
        accounting::aggregate_sol_balance(&accounts[USER_IDX], &accounts[USER_SOL_ATA_IDX])?;

    let net_sol = post_aggregate
        .checked_sub(pre_aggregate)
        .ok_or(arb_err(ARB_NEGATIVE_NET))?;
    if net_sol < min_profit {
        return Err(arb_err(ARB_INSUFFICIENT_PROFIT));
    }

    let meme_after = accounting::read_token_amount(&accounts[USER_MEME_ATA_IDX])?;
    if meme_after != meme_before {
        return Err(arb_err(ARB_RESIDUAL_MEME));
    }

    Ok(())
}

/// Identify a DEX by scanning program IDs at known offsets.
/// CPMM/Whirlpool/DLMM: program at section[0]. PumpSwap: program at section[16].
fn identify_dex(accounts: &[AccountInfo], base: usize) -> Option<DexKind> {
    // Try position 0 (CPMM, Whirlpool, DLMM)
    if let Some(kind) = DexKind::from_program_id(accounts[base].key) {
        return Some(kind);
    }
    // Try position 16 (PumpSwap buy and sell both have program at offset 16)
    if let Some(kind) = DexKind::from_program_id(accounts[base + 16].key) {
        return Some(kind);
    }
    None
}

/// Validate a DEX section by dispatching to the appropriate handler.
fn validate_dex_section(
    accounts: &[AccountInfo],
    kind: DexKind,
    base: usize,
    remaining: usize,
) -> ProgramResult {
    match kind {
        DexKind::PumpSwap => dex_pumpswap::validate_section(accounts, base, remaining),
        DexKind::Dlmm => dex_dlmm::validate_section(accounts, base, remaining),
        DexKind::Cpmm => dex_cpmm::validate_section(accounts, base, remaining),
        DexKind::Whirlpool => dex_whirlpool::validate_section(accounts, base, remaining),
    }
}

/// Execute the buy leg CPI (SOL → meme).
fn execute_buy_leg(
    accounts: &[AccountInfo],
    kind: DexKind,
    base: usize,
    remaining: usize,
    amount_in: u64,
    min_meme_out: u64,
    _sol_is_input: bool,
) -> ProgramResult {
    match kind {
        DexKind::PumpSwap => {
            let ix = dex_pumpswap::build_buy_cpi(
                accounts, base, remaining,
                amount_in, min_meme_out, false, None,
            );
            invoke(&ix, accounts)
                .map_err(|e| { msg!("PumpSwap buy CPI failed"); arb_err(crate::error::ARB_PUMP_CPI_FAILED) })
        }
        DexKind::Dlmm => {
            // DLMM section: 9 base + 4 extra (mints/programs at 9-12) + bin_arrays.
            // swap2 token_x = SOL (sorted), token_y = meme. a_to_b via swap2 params.
            let ix = dex_dlmm::build_swap2_cpi(
                accounts, base,
                amount_in,
                base + dex_dlmm::DLMM_TOKEN_X_MINT_OFF, // token_x_mint (SOL)
                base + dex_dlmm::DLMM_TOKEN_Y_MINT_OFF, // token_y_mint (meme)
                base + dex_dlmm::DLMM_TOKEN_X_PROG_OFF, // token_x_program
                base + dex_dlmm::DLMM_TOKEN_Y_PROG_OFF, // token_y_program
                USER_IDX,
                USER_SOL_ATA_IDX,   // user_token_in = SOL
                USER_MEME_ATA_IDX,  // user_token_out = meme
                remaining,          // bin_array_count
            );
            invoke(&ix, accounts)
                .map_err(|e| { msg!("DLMM buy CPI failed"); arb_err(crate::error::ARB_DLMM_CPI_FAILED) })
        }
        DexKind::Cpmm => {
            let ix = dex_cpmm::build_swap_cpi(
                accounts, base, amount_in, min_meme_out,
                USER_IDX,
                base + 8,  // input_mint (index 8 in CPMM section)
                base + 9,  // output_mint (index 9 in CPMM section)
                USER_SOL_ATA_IDX,  // input ATA (SOL)
                USER_MEME_ATA_IDX, // output ATA (meme)
                base + 10, // input_token_prog (index 10)
                base + 11, // output_token_prog (index 11)
                base + 12, // memo_program (index 12)
            );
            invoke(&ix, accounts)
                .map_err(|e| { msg!("CPMM buy CPI failed"); arb_err(crate::error::ARB_CPMM_CPI_FAILED) })
        }
        DexKind::Whirlpool => {
            let ix = dex_whirlpool::build_swap_cpi(
                accounts, base, remaining,
                amount_in, min_meme_out, true,
                base + dex_whirlpool::WHIRLPOOL_TOKEN_PROG_OFF,
                base + dex_whirlpool::WHIRLPOOL_TOKEN_PROG_OFF,
                USER_SOL_ATA_IDX,
                USER_MEME_ATA_IDX,
            );
            invoke(&ix, accounts)
                .map_err(|e| { msg!("Whirlpool buy CPI failed"); arb_err(crate::error::ARB_WHIRLPOOL_CPI_FAILED) })
        }
    }
}

/// Execute the sell leg CPI (meme → SOL).
fn execute_sell_leg(
    accounts: &[AccountInfo],
    kind: DexKind,
    base: usize,
    remaining: usize,
    amount_in: u64,
    min_sol_out: u64,
    _sol_is_output: bool,
) -> ProgramResult {
    match kind {
        DexKind::PumpSwap => {
            let ix = dex_pumpswap::build_sell_cpi(
                accounts, base, remaining,
                amount_in, min_sol_out, None,
            );
            invoke(&ix, accounts)
                .map_err(|e| { msg!("PumpSwap sell CPI failed"); arb_err(crate::error::ARB_PUMP_CPI_FAILED) })
        }
        DexKind::Dlmm => {
            let ix = dex_dlmm::build_swap2_cpi(
                accounts, base,
                amount_in,
                base + dex_dlmm::DLMM_TOKEN_X_MINT_OFF,
                base + dex_dlmm::DLMM_TOKEN_Y_MINT_OFF,
                base + dex_dlmm::DLMM_TOKEN_X_PROG_OFF,
                base + dex_dlmm::DLMM_TOKEN_Y_PROG_OFF,
                USER_IDX,
                USER_MEME_ATA_IDX,  // user_token_in = meme
                USER_SOL_ATA_IDX,   // user_token_out = SOL
                remaining,
            );
            invoke(&ix, accounts)
                .map_err(|e| { msg!("DLMM sell CPI failed"); arb_err(crate::error::ARB_DLMM_CPI_FAILED) })
        }
        DexKind::Cpmm => {
            let ix = dex_cpmm::build_swap_cpi(
                accounts, base, amount_in, min_sol_out,
                USER_IDX,
                base + 8,  // input_mint (meme for sell)
                base + 9,  // output_mint (SOL for sell)
                USER_MEME_ATA_IDX,
                USER_SOL_ATA_IDX,
                base + 10, // input_token_prog
                base + 11, // output_token_prog
                base + 12, // memo_program
            );
            invoke(&ix, accounts)
                .map_err(|e| { msg!("CPMM sell CPI failed"); arb_err(crate::error::ARB_CPMM_CPI_FAILED) })
        }
        DexKind::Whirlpool => {
            let ix = dex_whirlpool::build_swap_cpi(
                accounts, base, remaining,
                amount_in, min_sol_out, false,
                base + dex_whirlpool::WHIRLPOOL_TOKEN_PROG_OFF,
                base + dex_whirlpool::WHIRLPOOL_TOKEN_PROG_OFF,
                USER_MEME_ATA_IDX,
                USER_SOL_ATA_IDX,
            );
            invoke(&ix, accounts)
                .map_err(|e| { msg!("Whirlpool sell CPI failed"); arb_err(crate::error::ARB_WHIRLPOOL_CPI_FAILED) })
        }
    }
}
