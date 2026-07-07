//! Program error codes frozen at v0. Never renumber.
//!
//! Client maps each code to a structured `ErrorKind` for risk classification.

use solana_program::program_error::ProgramError;

// ── Post-CPI invariants ──────────────────────────────────────────────

/// `post_sol - pre_sol < min_profit_lamports`
pub const ARB_INSUFFICIENT_PROFIT: u32 = 6000;
/// `meme_after != meme_before` (residual meme position)
pub const ARB_RESIDUAL_MEME: u32 = 6001;
/// `amount_in_lamports == 0` or `min_profit_lamports == 0`
pub const ARB_ZERO_AMOUNT: u32 = 6002;

// ── Pre-CPI validation ───────────────────────────────────────────────

/// Discriminator does not match any known route.
pub const ARB_BAD_DISCRIMINATOR: u32 = 6003;
/// Account count mismatch vs. expected (route + remaining counts).
pub const ARB_BAD_ACCOUNT_COUNT: u32 = 6004;
/// PDA derivation mismatch.
pub const ARB_BAD_PDA: u32 = 6005;
/// Program ID at a fixed slot does not match the expected AMM/router.
pub const ARB_BAD_PROGRAM: u32 = 6006;
/// Quote mint is not NATIVE_SOL_MINT.
pub const ARB_BAD_MINT: u32 = 6007;
/// `post_sol < pre_sol` (checked_sub underflow).
pub const ARB_NEGATIVE_NET: u32 = 6008;
/// On-chain pre-swap pricing determined the trade would not be profitable.
pub const ARB_PRE_SWAP_UNPROFITABLE: u32 = 6009;
/// Arithmetic overflow during on-chain pricing.
pub const ARB_PRICE_MATH_OVERFLOW: u32 = 6010;
/// DLMM bin arrays have no bins with non-zero liquidity.
pub const ARB_EMPTY_BINS: u32 = 6011;

// ── CPI wrapper errors ───────────────────────────────────────────────

pub const ARB_PUMP_CPI_FAILED: u32 = 6100;
pub const ARB_DLMM_CPI_FAILED: u32 = 6200;
pub const ARB_CPMM_CPI_FAILED: u32 = 6300;
pub const ARB_WHIRLPOOL_CPI_FAILED: u32 = 6400;
/// No DEX handler matched the program IDs in the account list.
pub const ARB_UNKNOWN_DEX_PAIR: u32 = 6500;

// ── Mapping ──────────────────────────────────────────────────────────

/// Convert a custom error code to `ProgramError::Custom(code)`.
#[inline]
pub fn arb_err(code: u32) -> ProgramError {
    ProgramError::Custom(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_values() {
        assert_eq!(ARB_INSUFFICIENT_PROFIT, 6000);
        assert_eq!(ARB_RESIDUAL_MEME, 6001);
        assert_eq!(ARB_ZERO_AMOUNT, 6002);
        assert_eq!(ARB_BAD_DISCRIMINATOR, 6003);
        assert_eq!(ARB_BAD_ACCOUNT_COUNT, 6004);
        assert_eq!(ARB_BAD_PDA, 6005);
        assert_eq!(ARB_BAD_PROGRAM, 6006);
        assert_eq!(ARB_BAD_MINT, 6007);
        assert_eq!(ARB_NEGATIVE_NET, 6008);
        assert_eq!(ARB_PRE_SWAP_UNPROFITABLE, 6009);
        assert_eq!(ARB_PRICE_MATH_OVERFLOW, 6010);
        assert_eq!(ARB_EMPTY_BINS, 6011);
        assert_eq!(ARB_PUMP_CPI_FAILED, 6100);
        assert_eq!(ARB_DLMM_CPI_FAILED, 6200);
    }

    #[test]
    fn arb_err_maps_to_custom() {
        // arb_err wraps codes in ProgramError::Custom
        let err = arb_err(ARB_ZERO_AMOUNT);
        match err {
            ProgramError::Custom(code) => assert_eq!(code, ARB_ZERO_AMOUNT),
            _ => panic!("expected Custom"),
        }
    }

    /// Error codes 6000..=6008 must not overlap and must be distinct.
    #[test]
    fn error_codes_no_overlap_six_thousand() {
        let codes: [u32; 12] = [
            ARB_INSUFFICIENT_PROFIT,   // 6000
            ARB_RESIDUAL_MEME,         // 6001
            ARB_ZERO_AMOUNT,           // 6002
            ARB_BAD_DISCRIMINATOR,     // 6003
            ARB_BAD_ACCOUNT_COUNT,     // 6004
            ARB_BAD_PDA,               // 6005
            ARB_BAD_PROGRAM,           // 6006
            ARB_BAD_MINT,              // 6007
            ARB_NEGATIVE_NET,          // 6008
            ARB_PRE_SWAP_UNPROFITABLE, // 6009
            ARB_PRICE_MATH_OVERFLOW,   // 6010
            ARB_EMPTY_BINS,            // 6011
        ];
        for i in 0..codes.len() {
            for j in i + 1..codes.len() {
                assert_ne!(codes[i], codes[j], "error codes must be unique");
            }
        }
    }

    /// CPI wrapper errors are distinct from the 6000..=6008 range.
    #[test]
    fn cpi_error_codes_distinct() {
        assert_eq!(ARB_PUMP_CPI_FAILED, 6100);
        assert_eq!(ARB_DLMM_CPI_FAILED, 6200);
        assert_eq!(ARB_CPMM_CPI_FAILED, 6300);
        assert_eq!(ARB_WHIRLPOOL_CPI_FAILED, 6400);
        assert_ne!(ARB_PUMP_CPI_FAILED, ARB_DLMM_CPI_FAILED);
        assert_ne!(ARB_CPMM_CPI_FAILED, ARB_WHIRLPOOL_CPI_FAILED);
    }
}
