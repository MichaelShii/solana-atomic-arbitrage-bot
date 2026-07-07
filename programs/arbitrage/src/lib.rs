//! Atomic 2-leg arbitrage program (native solana-program, no Anchor).
//!
//! Two instructions:
//!   - `route_pump_to_dlmm` — PumpSwap buy → DLMM sell
//!   - `route_dlmm_to_pump` — DLMM buy → PumpSwap sell
//!
//! Stateless: no PDAs owned by the program, no admin, no upgrade authority
//! needed at runtime.

#![no_std]
#![allow(unexpected_cfgs)]

extern crate alloc;

#[allow(unused_imports)] // used by entrypoint! macro expansion in SBF builds
use alloc::format;
use solana_program::{
    account_info::AccountInfo, entrypoint, entrypoint::ProgramResult, program_error::ProgramError,
};

mod accounting;
mod constants;
mod cpi;
mod error;
mod instructions;
mod pricing;

use constants::{ROUTE_DLMM_TO_PUMP_DISC, ROUTE_PUMP_TO_DLMM_DISC, ROUTE_DISC};
use error::ARB_BAD_DISCRIMINATOR;

entrypoint!(process_instruction);

fn process_instruction(
    _program_id: &solana_program::pubkey::Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    if data.len() < 8 {
        return Err(ProgramError::InvalidInstructionData);
    }

    match data[..8].try_into().unwrap() {
        ROUTE_PUMP_TO_DLMM_DISC => instructions::route_pump_to_dlmm::handle(accounts, data),
        ROUTE_DLMM_TO_PUMP_DISC => instructions::route_dlmm_to_pump::handle(accounts, data),
        ROUTE_DISC => instructions::orchestrate::handle(accounts, data),
        _ => Err(error::arb_err(ARB_BAD_DISCRIMINATOR)),
    }
}
