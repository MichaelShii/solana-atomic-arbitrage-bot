//! On-chain program constants — Program IDs, mints, discriminators,
//! PDA seeds, and account index offsets.
//!
//! ## ⚠️ WHAT TO EDIT BEFORE DEPLOYMENT
//!
//! Only the `OUR_ARBITRAGE_PROGRAM_ID` values at the bottom of this file
//! need to be changed. They use `solana_program::pubkey!()` which requires
//! a valid base58 string at **compile time** (this is a Solana BPF constraint).
//!
//! Everything else — DEX IDs, mints, discriminators, PDA seeds, instruction
//! layout offsets — are public protocol constants. Do NOT change them.

use solana_program::pubkey::Pubkey;

// ── Program IDs ─────────────────────────────────────────────────────

pub const PUMP_SWAP_ID: Pubkey =
    solana_program::pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
pub const DLMM_ID: Pubkey = solana_program::pubkey!("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo");
pub const CPMM_ID: Pubkey =
    solana_program::pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
pub const WHIRLPOOL_ID: Pubkey =
    solana_program::pubkey!("whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc");
pub const TOKEN_ID: Pubkey = solana_program::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const SYSTEM_ID: Pubkey = solana_program::pubkey!("11111111111111111111111111111111");
pub const ATA_ID: Pubkey = solana_program::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const FEE_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
pub const MEMO_ID: Pubkey = solana_program::pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
/// DLMM global event authority (same for all pools).
pub const DLMM_EVENT_AUTH: Pubkey =
    solana_program::pubkey!("D1ZN9Wj1fRSUQfCjhvnu1hqDMT7hzjzBBpi12nVniYD6");

// ── Mints ────────────────────────────────────────────────────────────

pub const NATIVE_SOL_MINT: Pubkey =
    solana_program::pubkey!("So11111111111111111111111111111111111111112");

// ── PDA seeds ────────────────────────────────────────────────────────

pub const PUMP_EVENT_AUTH_SEED: &[u8] = b"__event_authority";
pub const PUMP_GLOBAL_CONFIG_SEED: &[u8] = b"global_config";
pub const PUMP_FEE_CONFIG_SEED: &[u8] = b"fee_config";
pub const DLMM_ORACLE_SEED: &[u8] = b"oracle";
/// CPMM authority PDA seed: vault_and_lp.
pub const CPMM_AUTH_SEED: &[u8] = b"vault_and_lp";
/// Whirlpool token authority PDA seed.
pub const WHIRLPOOL_AUTH_SEED: &[u8] = b"authority";
/// Whirlpool tick array PDA seed.
pub const WHIRLPOOL_TICK_ARRAY_SEED: &[u8] = b"tick_array";
/// Raydium CPMM known amm_config used for PDA derivation (config index 0).
pub const CPMM_AMM_CONFIG: Pubkey =
    solana_program::pubkey!("D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2");
/// Whirlpool config (base config used for whirlpool PDA derivation).
pub const WHIRLPOOL_CONFIG: Pubkey =
    solana_program::pubkey!("2LecshUwdy9xi7meFgHtFJQNSKk4KdTrcpvaB56dP2NQ");

// ── Discriminators ───────────────────────────────────────────────────

/// sha256("global:route_pump_to_dlmm")[..8]
pub const ROUTE_PUMP_TO_DLMM_DISC: [u8; 8] = [0x8b, 0xe8, 0x20, 0x55, 0xc1, 0xb0, 0xc1, 0xe9];

/// sha256("global:route_dlmm_to_pump")[..8]
pub const ROUTE_DLMM_TO_PUMP_DISC: [u8; 8] = [0x17, 0x6e, 0xcc, 0x5d, 0xdd, 0x93, 0x51, 0x95];

/// PumpSwap buy_exact_quote_in discriminator (official IDL).
pub const PUMP_BUY_DISC: [u8; 8] = [0xc6, 0x2e, 0x15, 0x52, 0xb4, 0xd9, 0xe8, 0x70];

/// PumpSwap sell discriminator (official IDL).
pub const PUMP_SELL_DISC: [u8; 8] = [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad];

/// DLMM swap2 discriminator (verified against mainnet CPI).
pub const DLMM_SWAP2_DISC: [u8; 8] = [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88];
/// CPMM swap discriminator (Anchor: sha256("global:swap")[..8]).
pub const CPMM_SWAP_DISC: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];
/// Whirlpool swap discriminator (Anchor: sha256("global:swap")[..8], same as CPMM).
pub const WHIRLPOOL_SWAP_DISC: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];

// ── Discriminators for generic route dispatcher ──────────────────────

/// sha256("global:arbitrage_route")[..8]
pub const ROUTE_DISC: [u8; 8] = [0x5f, 0x0f, 0x91, 0x02, 0x9a, 0x03, 0x4c, 0xc3];

// ── Instruction data layout (36 bytes) ──────────────────────────────
// Legacy format (route_pump_to_dlmm / route_dlmm_to_pump):
//  [disc(8) | amount_in_lamports(8) | min_profit_lamports(8) |
//   min_intermediate_meme_out(8) | track_volume(1) | dlmm_sol_is_x(1) |
//   pump_remaining_count(1) | dlmm_bin_array_count(1)]
//
// Generic format (ROUTE_DISC):
//  [disc(8) | amount_in_lamports(8) | min_profit_lamports(8) |
//   min_intermediate_meme_out(8) | track_volume(1) | buy_sol_is_x(1) |
//   buy_remaining_count(1) | sell_remaining_count(1)]

pub const OFF_AMOUNT_IN: usize = 8;
pub const OFF_MIN_PROFIT: usize = 16;
pub const OFF_MIN_INTERMEDIATE: usize = 24;
pub const OFF_TRACK_VOLUME: usize = 32;
pub const OFF_DLMM_SOL_IS_X: usize = 33;
pub const OFF_PUMP_REMAINING: usize = 34;
pub const OFF_DLMM_BIN_ARRAY_COUNT: usize = 35;
/// Generic: buy-side remaining account count (same offset as OFF_PUMP_REMAINING).
pub const OFF_BUY_REMAINING: usize = 34;
/// Generic: sell-side remaining account count.
pub const OFF_SELL_REMAINING: usize = 35;
pub const IX_DATA_LEN: usize = 36;

// ── DLMM lb_pair account field offsets (verified against IDL) ─────────

/// active_id: i32 at offset 76
pub const LB_PAIR_OFF_ACTIVE_ID: usize = 76;
/// bin_step: u16 at offset 80
pub const LB_PAIR_OFF_BIN_STEP: usize = 80;
/// base_factor: u16 at offset 84
pub const LB_PAIR_OFF_BASE_FACTOR: usize = 84;
/// token_x_mint: Pubkey at offset 88
pub const LB_PAIR_OFF_TOKEN_X_MINT: usize = 88;
/// token_y_mint: Pubkey at offset 120
pub const LB_PAIR_OFF_TOKEN_Y_MINT: usize = 120;
/// Minimum lb_pair data size for field access
#[allow(dead_code)]
pub const LB_PAIR_MIN_LEN: usize = 152;

// ── Shared user account indices (both routes) ────────────────────────

pub const USER_IDX: usize = 0;
pub const USER_SOL_ATA_IDX: usize = 1;
pub const USER_MEME_ATA_IDX: usize = 2;
pub const SHARED_FIXED_LEN: usize = 3;

// ── route_pump_to_dlmm: PumpSwap Buy section (after shared) ─────────
// Indices are relative to PUMP_BASE (which equals SHARED_FIXED_LEN = 3).

pub const PUMP_BUY_POOL: usize = 0;
pub const PUMP_BUY_USER: usize = 1; // == accounts[USER_IDX]
pub const PUMP_BUY_GLOBAL_CONFIG: usize = 2;
pub const PUMP_BUY_BASE_MINT: usize = 3;
pub const PUMP_BUY_QUOTE_MINT: usize = 4;
pub const PUMP_BUY_USER_BASE_ATA: usize = 5; // == accounts[USER_MEME_ATA_IDX]
pub const PUMP_BUY_USER_QUOTE_ATA: usize = 6; // == accounts[USER_SOL_ATA_IDX]
pub const PUMP_BUY_POOL_BASE_ATA: usize = 7;
pub const PUMP_BUY_POOL_QUOTE_ATA: usize = 8;
pub const PUMP_BUY_PROTOCOL_FEE_RECIPIENT: usize = 9;
pub const PUMP_BUY_PROTOCOL_FEE_ATA: usize = 10;
pub const PUMP_BUY_BASE_TOKEN_PROGRAM: usize = 11;
pub const PUMP_BUY_QUOTE_TOKEN_PROGRAM: usize = 12;
pub const PUMP_BUY_SYSTEM_PROGRAM: usize = 13;
pub const PUMP_BUY_ATA_PROGRAM: usize = 14;
pub const PUMP_BUY_EVENT_AUTHORITY: usize = 15;
pub const PUMP_BUY_PROGRAM: usize = 16;
pub const PUMP_BUY_COIN_CREATOR_VAULT_ATA: usize = 17;
pub const PUMP_BUY_COIN_CREATOR_VAULT_AUTH: usize = 18;
pub const PUMP_BUY_GLOBAL_VOL_ACCUM: usize = 19;
pub const PUMP_BUY_USER_VOL_ACCUM: usize = 20;
pub const PUMP_BUY_FEE_CONFIG: usize = 21;
pub const PUMP_BUY_FEE_PROGRAM: usize = 22;
pub const PUMP_BUY_FIXED_LEN: usize = 23;

// ── route_pump_to_dlmm: DLMM section offsets (relative to dlmm_base) ─
// dlmm_base = PUMP_BASE + PUMP_BUY_FIXED_LEN + pump_remaining_count

pub const DLMM_PROGRAM_REL: usize = 0;
pub const DLMM_LB_PAIR_REL: usize = 1;
pub const DLMM_BITMAP_REL: usize = 2;
pub const DLMM_RESERVE_X_REL: usize = 3;
pub const DLMM_RESERVE_Y_REL: usize = 4;
pub const DLMM_ORACLE_REL: usize = 5;
pub const DLMM_HOST_FEE_REL: usize = 6;
/// Memo program at dlmm_base+7 (validated against mainnet R2-H02).
pub const DLMM_MEMO_REL: usize = 7;
/// Event authority at dlmm_base+8 (validated against mainnet R2-H02).
pub const DLMM_EVENT_AUTH_REL: usize = 8;
pub const DLMM_BIN_ARRAYS_START_REL: usize = 9;
pub const DLMM_FIXED_LEN: usize = 9;

// ── route_dlmm_to_pump: PumpSwap Sell section (after shared + DLMM) ─
// Indices are relative to pump_sell_base.
// pump_sell_base = SHARED_FIXED_LEN + DLMM_FIXED_LEN + dlmm_bin_array_count

pub const PUMP_SELL_POOL: usize = 0;
pub const PUMP_SELL_USER: usize = 1; // == accounts[USER_IDX]
pub const PUMP_SELL_GLOBAL_CONFIG: usize = 2;
pub const PUMP_SELL_BASE_MINT: usize = 3;
pub const PUMP_SELL_QUOTE_MINT: usize = 4;
pub const PUMP_SELL_USER_BASE_ATA: usize = 5; // == accounts[USER_MEME_ATA_IDX]
pub const PUMP_SELL_USER_QUOTE_ATA: usize = 6; // == accounts[USER_SOL_ATA_IDX]
pub const PUMP_SELL_POOL_BASE_ATA: usize = 7;
pub const PUMP_SELL_POOL_QUOTE_ATA: usize = 8;
pub const PUMP_SELL_PROTOCOL_FEE_RECIPIENT: usize = 9;
pub const PUMP_SELL_PROTOCOL_FEE_ATA: usize = 10;
pub const PUMP_SELL_BASE_TOKEN_PROGRAM: usize = 11;
pub const PUMP_SELL_QUOTE_TOKEN_PROGRAM: usize = 12;
pub const PUMP_SELL_SYSTEM_PROGRAM: usize = 13;
pub const PUMP_SELL_ATA_PROGRAM: usize = 14;
pub const PUMP_SELL_EVENT_AUTHORITY: usize = 15;
pub const PUMP_SELL_PROGRAM: usize = 16;
pub const PUMP_SELL_COIN_CREATOR_VAULT_ATA: usize = 17;
pub const PUMP_SELL_COIN_CREATOR_VAULT_AUTH: usize = 18;
pub const PUMP_SELL_FEE_CONFIG: usize = 19;
pub const PUMP_SELL_FEE_PROGRAM: usize = 20;
pub const PUMP_SELL_FIXED_LEN: usize = 21;

// ── DEX handler kind identifiers ─────────────────────────────────────

pub const DEX_KIND_PUMPSWAP: u8 = 0;
pub const DEX_KIND_DLMM: u8 = 1;
pub const DEX_KIND_CPMM: u8 = 2;
pub const DEX_KIND_WHIRLPOOL: u8 = 3;

// ── Generic route: DEX section account layout ────────────────────────
// Each DEX section has its program ID at a fixed offset.
// Share account layout:
//   [0] user (signer), [1] user_sol_ata, [2] user_meme_ata
// Buy section starts at offset SHARED_FIXED_LEN.
// Sell section starts after buy section.
// Within each section, DEX_PROGRAM_OFF (0) is the DEX program ID.
pub const DEX_PROGRAM_OFF: usize = 0;
/// Minimum account count: shared(3) + buy_fixed(>=1) + sell_fixed(>=1)
pub const GENERIC_MIN_ACCTS: usize = 5;

// ============================================================
// ⚠️  YOUR PROGRAM ID — MUST EDIT BEFORE DEPLOYMENT
// ============================================================
// Replace "11111111111111111111111111111111" with your actual
// deployed arbitrage program address BEFORE running `cargo build-sbf`.
// This uses `solana_program::pubkey!()` macro — a compile-time constant.
// The client side reads this from config.toml → [execution_routing].
// ============================================================

// TODO: Replace with your deployed arbitrage program ID before building with --features mainnet
#[cfg(feature = "mainnet")]
#[allow(dead_code)]
pub const OUR_ARBITRAGE_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("11111111111111111111111111111111");

// TODO: Replace with your devnet program ID
#[cfg(not(feature = "mainnet"))]
#[allow(dead_code)]
pub const OUR_ARBITRAGE_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("11111111111111111111111111111111");

#[cfg(test)]
#[path = "constants_tests.rs"]
mod tests;
