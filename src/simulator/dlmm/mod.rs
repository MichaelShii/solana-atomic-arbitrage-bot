//! Meteora DLMM Swap2 instruction builder

use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::{DLMM_PROGRAM, DLMM_SWAP2_DISCRIMINATOR};
use crate::pool_cache::DlmmBin;

/// Build Meteora DLMM Swap2 instruction (account layout verified against IDL 2026-06)
///
/// Account order:
///   1. lb_pair                 (writable)
///   2. bin_array_bitmap_extension (readonly, optional — DLMM_PROGRAM placeholder when None)
///   3. reserve_x               (writable)
///   4. reserve_y               (writable)
///   5. user_token_in           (writable)
///   6. user_token_out          (writable)
///   7. token_x_mint            (readonly)
///   8. token_y_mint            (readonly)
///   9. oracle                  (writable)
///  10. host_fee_in             (writable, optional — DLMM_PROGRAM placeholder when None)
///  11. user                    (signer)
///  12. token_x_program         (readonly) — Tokenkeg or Token-2022
///  13. token_y_program         (readonly) — Tokenkeg or Token-2022
///  14. memo_program            (readonly)
///  15. event_authority         (readonly)
///  16. event_program           (readonly) — DLMM program itself
///
/// Remaining accounts: bin arrays (readonly)
#[allow(clippy::too_many_arguments)]
pub fn build_dlmm_swap2_ix(
    user: &Pubkey,
    lb_pair: &Pubkey,
    bin_arrays: &[Pubkey],
    reserve_x: &Pubkey,
    reserve_y: &Pubkey,
    user_token_in: &Pubkey,
    user_token_out: &Pubkey,
    token_x_mint: &Pubkey,
    token_y_mint: &Pubkey,
    oracle: &Pubkey,
    event_authority: &Pubkey,
    amount_in: u64,
    min_amount_out: u64,
    token_x_program: &Pubkey,
    token_y_program: &Pubkey,
    memo_program: &Pubkey,
    event_program: &Pubkey,
    bin_array_bitmap_extension: Option<&Pubkey>,
    host_fee_in: Option<&Pubkey>,
) -> Instruction {
    let dlmm = Pubkey::from_str(DLMM_PROGRAM).unwrap();

    let mut accounts: Vec<AccountMeta> = Vec::with_capacity(18 + bin_arrays.len());

    // 1. lb_pair (writable)
    accounts.push(AccountMeta::new(*lb_pair, false));

    // 2. bin_array_bitmap_extension (writable since DLMM v0.12.0, optional)
    // Always included at fixed position — verified against real on-chain Swap2 tx.
    // DLMM program ignores this account when lb_pair has no bitmap extension,
    // but the account must be present to keep subsequent index positions correct.
    accounts.push(AccountMeta::new(
        *bin_array_bitmap_extension.unwrap_or(&dlmm),
        false,
    ));

    // 3-4. reserve_x, reserve_y (writable)
    accounts.push(AccountMeta::new(*reserve_x, false));
    accounts.push(AccountMeta::new(*reserve_y, false));

    // 5-6. user_token_in, user_token_out (writable)
    accounts.push(AccountMeta::new(*user_token_in, false));
    accounts.push(AccountMeta::new(*user_token_out, false));

    // 7-8. token_x_mint, token_y_mint (readonly)
    accounts.push(AccountMeta::new_readonly(*token_x_mint, false));
    accounts.push(AccountMeta::new_readonly(*token_y_mint, false));

    // 9. oracle (writable — required for price feed updates)
    accounts.push(AccountMeta::new(*oracle, false));

    // 10. host_fee_in (writable, optional)
    // Always included at fixed position — same reason as bitmap_extension.
    // DLMM program ignores when no host fee is configured.
    accounts.push(AccountMeta::new(*host_fee_in.unwrap_or(&dlmm), false));

    // 11. user (signer)
    accounts.push(AccountMeta::new_readonly(*user, true));

    // 12-13. token_x_program, token_y_program (readonly)
    accounts.push(AccountMeta::new_readonly(*token_x_program, false));
    accounts.push(AccountMeta::new_readonly(*token_y_program, false));

    // 14. memo_program (readonly)
    accounts.push(AccountMeta::new_readonly(*memo_program, false));

    // 15. event_authority (readonly)
    accounts.push(AccountMeta::new_readonly(*event_authority, false));

    // 16. event_program (readonly — the DLMM program itself)
    accounts.push(AccountMeta::new_readonly(*event_program, false));

    // remaining: bin arrays (writable — DLMM updates bin liquidity during swap)
    for ba in bin_arrays {
        accounts.push(AccountMeta::new(*ba, false));
    }

    // Data layout (28 bytes):
    // [0..8]   discriminator              [u8; 8]
    // [8..16]  amount_in                  u64 LE
    // [16..24] min_amount_out             u64 LE
    // [24..28] remaining_accounts_info    { slices: Vec<RemainingAccountsSlice> }
    //                                    Borsh-encoded empty vec = [0,0,0,0] (u32 LE)
    let mut data = Vec::with_capacity(28);
    data.extend_from_slice(&DLMM_SWAP2_DISCRIMINATOR);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes()); // empty remaining_accounts_info

    Instruction {
        program_id: dlmm,
        accounts,
        data,
    }
}

/// Per-bin DLMM swap output estimation (replaces global CPMM approximation)
///
/// DLMM uses a piecewise constant-product model: each bin has independent reserve_x/reserve_y,
/// and a swap consumes bins sequentially in price direction. A global CPMM approximation
/// can be off by 20-50% for multi-bin trades.
///
/// Algorithm: sort bins by bin_id → consume from the current active bin forward →
///   within each bin use constant-product: output = reserve_out * amount_in / (reserve_in + amount_in)
/// Fee parameter is applied to input amount (e.g., 0.0025 for 0.25%)
/// Result of a DLMM swap estimation.
pub struct DlmmEstimate {
    pub out: u64,
    pub bins_consumed: u32,
    pub bins_total: u32,
    /// Bin IDs and amounts actually consumed during the estimate (first 3).
    pub consumed_bins: Vec<(i32, u64, u64)>, // (bin_id, reserve_in, reserve_out)
}

pub fn estimate_dlmm_swap_output(
    bins: &[DlmmBin],
    amount_in: u64,
    is_x_to_y: bool,
    fee: f64,
) -> u64 {
    estimate_dlmm_swap_output_full(bins, amount_in, is_x_to_y, fee).out
}

pub fn estimate_dlmm_swap_output_full(
    bins: &[DlmmBin],
    amount_in: u64,
    is_x_to_y: bool,
    fee: f64,
) -> DlmmEstimate {
    let fee_bps = (fee * 10000.0) as u128;
    let amount_after_fee = (amount_in as u128 * (10000 - fee_bps) / 10000) as u64;
    if bins.is_empty() || amount_after_fee == 0 {
        return DlmmEstimate { out: 0, bins_consumed: 0, bins_total: 0, consumed_bins: vec![] };
    }

    // Collect bins with non-zero amounts, sorted by bin_id.
    // amount_x/amount_y are the actual available reserves (DLMM Bin has no reserve fields).
    let mut active: Vec<&DlmmBin> = bins
        .iter()
        .filter(|b| b.amount_x > 0 || b.amount_y > 0)
        .collect();

    // X→Y: higher bin_id first (selling X for Y — higher bin_id = more Y per X)
    // Y→X: lower bin_id first (selling Y for X — lower bin_id = cheaper X per Y)
    if is_x_to_y {
        active.sort_by_key(|b| std::cmp::Reverse(b.bin_id));
    } else {
        active.sort_by_key(|b| b.bin_id);
    }

    let mut remaining_in = amount_after_fee as u128;
    let mut total_out: u128 = 0;
    let mut bins_consumed: u32 = 0;
    let mut consumed_bins: Vec<(i32, u64, u64)> = Vec::new();

    for bin in &active {
        let (reserve_in, reserve_out) = if is_x_to_y {
            (bin.amount_x as u128, bin.amount_y as u128)
        } else {
            (bin.amount_y as u128, bin.amount_x as u128)
        };

        if reserve_in == 0 || reserve_out == 0 {
            continue;
        }

        bins_consumed += 1;
        if consumed_bins.len() < 3 {
            consumed_bins.push((bin.bin_id, reserve_in as u64, reserve_out as u64));
        }

        if remaining_in <= reserve_in {
            // Partial fill: this bin is a limit order at fixed price.
            // Proportionally consume reserve_in and receive reserve_out.
            total_out += reserve_out * remaining_in / reserve_in;
            break;
        } else {
            // Exhaust this bin: consume all reserve_in, receive all reserve_out.
            total_out += reserve_out;
            remaining_in -= reserve_in;
        }
    }

    let ratio = if amount_in > 0 {
        total_out as f64 / amount_after_fee as f64
    } else {
        0.0
    };
    log::debug!(
        "[DLMM-EST] done amount_in={} amount_after_fee={} is_x_to_y={} total_out={} bins_consumed={} bins_total={} ratio={:.3}",
        amount_in, amount_after_fee, is_x_to_y, total_out, bins_consumed, active.len(), ratio,
    );

    DlmmEstimate {
        out: total_out as u64,
        bins_consumed,
        bins_total: active.len() as u32,
        consumed_bins,
    }
}

#[cfg(test)]
mod tests;
