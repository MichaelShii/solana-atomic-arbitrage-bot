//! Pre/post balance helpers. Reads SPL token amounts from account data
//! and aggregates SOL + WSOL for net SOL tracking.

use solana_program::{account_info::AccountInfo, program_error::ProgramError};

/// Read SPL token amount from account data at offset 64.
/// Works for both Token and Token-2022 (base layout offset 64 is
/// identical for both standards).
pub fn read_token_amount(account: &AccountInfo) -> Result<u64, ProgramError> {
    let data = account.data.borrow();
    if data.len() < 72 {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(u64::from_le_bytes(data[64..72].try_into().unwrap()))
}

/// Aggregate SOL balance: native SOL lamports in the user wallet plus
/// WSOL token amount in the user's WSOL ATA. Used for net SOL tracking
/// across both wrapped and unwrapped settlement paths.
pub fn aggregate_sol_balance(
    user_wallet: &AccountInfo,
    user_wsol_ata: &AccountInfo,
) -> Result<u64, ProgramError> {
    let native = user_wallet.lamports();
    let wsol = read_token_amount(user_wsol_ata)?;
    native
        .checked_add(wsol)
        .ok_or(ProgramError::ArithmeticOverflow)
}
