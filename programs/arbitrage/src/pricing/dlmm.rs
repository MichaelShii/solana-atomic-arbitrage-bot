//! Pre-swap DLMM pricing: parse bin arrays, walk bins in price order.

use alloc::vec::Vec;
use solana_program::account_info::AccountInfo;
use solana_program::program_error::ProgramError;

use crate::error::{arb_err, ARB_EMPTY_BINS, ARB_PRICE_MATH_OVERFLOW};

const BIN_ARRAY_HEADER_SIZE: usize = 8 + 48;

#[inline]
fn read_bin_amounts(data: &[u8], base_offset: usize) -> Option<(u64, u64)> {
    if base_offset + 16 > data.len() {
        return None;
    }
    let ax = u64::from_le_bytes(data[base_offset..base_offset + 8].try_into().ok()?);
    let ay = u64::from_le_bytes(data[base_offset + 8..base_offset + 16].try_into().ok()?);
    Some((ax, ay))
}

/// Estimate DLMM swap output by walking bins in price order.
/// Uses Vec for efficient O(n log n) sorting (timsort).
pub fn estimate_dlmm_swap_output(
    bin_arrays: &[&AccountInfo],
    amount_in: u64,
    is_x_to_y: bool,
) -> Result<u64, ProgramError> {
    if amount_in == 0 {
        return Ok(0);
    }

    let mut bins: Vec<(i32, u64, u64)> = Vec::new();
    for arr in bin_arrays {
        let data = arr.data.borrow();
        if data.len() < BIN_ARRAY_HEADER_SIZE {
            continue;
        }
        let version = data.get(16).copied().unwrap_or(1);
        let stride: usize = match version {
            1 => 128,
            2 => 144,
            _ => continue,
        };
        let arr_index = i64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));

        for bin_i in 0..70 {
            let offset = BIN_ARRAY_HEADER_SIZE + bin_i * stride;
            let (ax, ay) = match read_bin_amounts(&data, offset) {
                Some(v) => v,
                None => break,
            };
            let (reserve_in, reserve_out) = if is_x_to_y {
                (ax, ay)
            } else {
                (ay, ax)
            };
            if reserve_in > 0 && reserve_out > 0 {
                let bin_id = arr_index.saturating_mul(70).saturating_add(bin_i as i64) as i32;
                bins.push((bin_id, reserve_in, reserve_out));
            }
        }
    }

    if bins.is_empty() {
        return Err(arb_err(ARB_EMPTY_BINS));
    }

    if is_x_to_y {
        bins.sort_by(|a, b| b.0.cmp(&a.0));
    } else {
        bins.sort_by(|a, b| a.0.cmp(&b.0));
    }

    let mut remaining_in: u128 = amount_in as u128;
    let mut total_out: u128 = 0;

    for (_bin_id, ri, ro) in &bins {
        let ri = *ri as u128;
        let ro = *ro as u128;

        if remaining_in <= ri {
            let num = ro
                .checked_mul(remaining_in)
                .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))?;
            let den = ri
                .checked_add(remaining_in)
                .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))?;
            total_out = total_out
                .checked_add(num.checked_div(den).unwrap_or(0))
                .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))?;
            remaining_in = 0;
            break;
        } else {
            let num = ro
                .checked_mul(ri)
                .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))?;
            let den = ri
                .checked_add(ri)
                .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))?;
            total_out = total_out
                .checked_add(num.checked_div(den).unwrap_or(0))
                .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))?;
            remaining_in = remaining_in
                .checked_sub(ri)
                .ok_or(arb_err(ARB_PRICE_MATH_OVERFLOW))?;
        }
    }

    Ok(total_out as u64)
}
