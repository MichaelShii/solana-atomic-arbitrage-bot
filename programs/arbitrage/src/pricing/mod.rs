//! On-chain pre-swap pricing: CPMM (PumpSwap) and bin-array (DLMM).
//! All math uses u128 checked arithmetic — no floats in SBF.

pub mod dlmm;
pub mod pump_swap;

/// Compute constant-product AMM swap output using additive-fee formula.
///
/// `effective = amount_in * 10000 / (10000 + fee_bps)`
/// `output = reserve_out * effective / (reserve_in + effective)`
///
/// Returns `(output_amount, fee_amount)` or `None` on overflow or zero.
#[inline]
pub fn cpmm_swap_output(
    reserve_in: u64,
    reserve_out: u64,
    amount_in: u64,
    fee_bps: u16,
) -> Option<(u64, u64)> {
    let ri = reserve_in as u128;
    let ro = reserve_out as u128;
    let a = amount_in as u128;
    let fee = fee_bps as u128;

    if a == 0 || ri == 0 || ro == 0 {
        return None;
    }

    // effective = amount_in * 10000 / (10000 + fee_bps)
    let denom = 10_000u128.checked_add(fee)?;
    let num = a.checked_mul(10_000u128)?;
    let effective = num.checked_div(denom)?;
    if effective == 0 {
        return None;
    }

    let fee_amount = a.checked_sub(effective)?;
    let numerator = ro.checked_mul(effective)?;
    let denominator = ri.checked_add(effective)?;
    let output = numerator.checked_div(denominator)?;
    if output == 0 {
        return None;
    }

    Some((output as u64, fee_amount as u64))
}
