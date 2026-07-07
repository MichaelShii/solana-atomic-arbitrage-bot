//! Global constants for Solana protocol addresses, mints, and discriminators.
//!
//! ## IMPORTANT: What to edit vs what to leave alone
//!
//! **Everything in this file is a public protocol constant.** None of these
//! values are specific to your deployment — they are well-known Solana
//! mainnet addresses that are the same for everyone.
//!
//! **Values you need to configure are in `config.toml`:**
//! - Your RPC endpoint → `[solana].rpc_url`
//! - Your wallet keypair → `[wallet].keypair_path` or `BOT_PRIVATE_KEY` env
//! - Your deployed program ID → `[execution_routing].onchain_program_id`
//! - Profit thresholds, slippage, etc. → `[risk]` / `[scanner]` sections
//!
//! See `.env.example` + `config.example.toml` for the full list.
//!
//! Some constants below carry `#[allow(dead_code)]` because they are
//! reserved for future venue expansion (AMMv4, Serum, etc.) and are
//! referenced by code gated behind those features.

// ============================================================
// PUBLIC PROTOCOL ADDRESSES — Solana mainnet, same for everyone
// You should NEVER need to edit these.
// ============================================================

// ---- DEX Program IDs ----
pub const CPMM_PROGRAM: &str = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";
pub const AMM_V4_PROGRAM: &str = "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8";
/// PumpSwap AMM program — graduated token pools (Pool creation, swap, migrate)
pub const PUMPFUN_AMM_PROGRAM: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";
/// Pump.fun bonding curve program — pre-graduation buy/sell
pub const PUMPFUN_BONDING_CURVE_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
pub const DLMM_PROGRAM: &str = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";
pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const TOKEN22_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
pub const MEMO_PROGRAM: &str = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr";
pub const ATA_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
pub const SERUM_PROGRAM: &str = "srmqPvymJeFKQ4zGQed1GFppgkRHL9kaELCbyksJtPX";
pub const WHIRLPOOL_PROGRAM: &str = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

// ---- Sysvars & Token Programs ----
pub const SYSVAR_RENT: &str = "SysvarRent111111111111111111111111111111111";

// ---- Mint Addresses ----
pub const NATIVE_SOL_MINT: &str = "So11111111111111111111111111111111111111112";
pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const USDT_MINT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";

// ---- Decimals ----
pub const SOL_DECIMALS: u8 = 9;
pub const USDC_DECIMALS: u8 = 6;
/// Most memecoins / SPL tokens use 6 decimals. SOL itself is 9.
pub const DEFAULT_DECIMALS: u8 = 6;

// ============================================================
// PROTOCOL BINARY CONSTANTS — Discriminators & PDAs
// These are sha256 hashes and PDA seeds. Changing any value here
// will break instruction construction and cause TX failures.
// ============================================================

// ---- Well-known Config PDAs ----
pub const CPMM_AMM_CONFIG: &str = "D4FPEruKEHrG5TenZ2mpDGEfu1iUvTiqBxvpU8HLBvC2";
pub const WHIRLPOOL_CONFIG: &str = "2LecshUwdy9xi7meFgHtFJQNSKk4KdTrcpvaB56dP2NQ";
pub const CPMM_SWAP_DISCRIMINATOR: [u8; 8] = [0xf8, 0xc6, 0x9e, 0x91, 0xe1, 0x75, 0x87, 0xc8];
pub const AMMV4_SWAP_DISCRIMINATOR: [u8; 8] = [0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
pub const AMMV4_SWAP_OUT_DISCRIMINATOR: [u8; 8] = [0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
pub const PUMPFUN_BUY_DISCRIMINATOR: [u8; 8] = [0x67, 0xf4, 0x52, 0x1f, 0x2c, 0xf5, 0x77, 0x77];
pub const PUMPFUN_SELL_DISCRIMINATOR: [u8; 8] = [0x3e, 0x2f, 0x37, 0x0a, 0xa5, 0x03, 0xdc, 0x2a];
/// PumpSwap AMM buy_exact_quote_in discriminator (verified against official SDK IDL)
pub const PUMPSWAP_BUY_DISCRIMINATOR: [u8; 8] = [0xc6, 0x2e, 0x15, 0x52, 0xb4, 0xd9, 0xe8, 0x70];
/// PumpSwap AMM sell discriminator (verified against official SDK IDL)
pub const PUMPSWAP_SELL_DISCRIMINATOR: [u8; 8] = [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad];
pub const DLMM_SWAP2_DISCRIMINATOR: [u8; 8] = [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88];

// ---- DEX Event Authorities & Oracles ----
/// DLMM event authority — global address (same across all pools), verified from real Swap2 txs.
pub const DLMM_EVENT_AUTHORITY: &str = "D1ZN9Wj1fRSUQfCjhvnu1hqDMT7hzjzBBpi12nVniYD6";
/// DLMM oracle (PDA seed = "oracle", derived from lb_pair).
pub const DLMM_ORACLE: &str = "5DmhHY4YTvqRMTaL1tPBWY5S8J4oCLq3sCMbvrvnZKu8";
/// PumpSwap AMM event authority PDA (seed = "__event_authority")
pub const PUMPSWAP_EVENT_AUTHORITY: &str = "GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR";
/// PumpSwap AMM global config PDA (seed = "global_config")
pub const PUMPSWAP_GLOBAL_CONFIG: &str = "ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw";
/// PumpSwap fee program that owns fee_config
pub const PUMPSWAP_FEE_PROGRAM: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";
/// PumpSwap global volume accumulator PDA (seed = "global_volume_accumulator")
pub const PUMPSWAP_GLOBAL_VOLUME_ACCUMULATOR: &str = "C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw";
/// PumpSwap protocol fee recipients (pick any; the on-chain program accepts all 8).
/// Used for normal (non-Mayhem) pools.
pub const PUMPSWAP_PROTOCOL_FEE_RECIPIENTS: [&str; 8] = [
    "62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV",
    "7VtfL8fvgNfhz17qKRMjzQEXgbdpnHHHQRh54R9jP2RJ",
    "7hTckgnGnLQR6sdH7YkqFTAA7VwTfYFaZ6EhEsU3saCX",
    "9rPYyANsfQZw3DnDmKE3YCQF5E8oD89UXoHn9JFEhJUz",
    "AVmoTthdrX6tKt4nDjco2D775W2YK3sDhxPcMmzUAmTY",
    "FWsW1xNtWscwNmKv6wVsU1iTzRN6wmmk3MjxRP5tT7hz",
    "G5UZAVbAf46s7cKWoyKu8kYTip9DGTpbLZ2qa9Aq69dP",
    "JCRGumoE9Qi5BBgULTgdgTLjSgkCMSbF62ZZfGs84JeU",
];
/// PumpSwap reserved fee recipients for Mayhem-mode pools.
pub const PUMPSWAP_RESERVED_FEE_RECIPIENTS: [&str; 8] = [
    "GesfTA3X2arioaHp8bbKdjG9vJtskViWACZoYvxp4twS",
    "4budycTjhs9fD6xw62VBducVTNgMgJJ5BgtKq7mAZwn6",
    "8SBKzEQU4nLSzcwF4a74F2iaUDQyTfjGndn6qUWBnrpR",
    "4UQeTP1T39KZ9Sfxzo3WR5skgsaP6NZa87BAkuazLEKH",
    "8sNeir4QsLsJdYpc9RZacohhK1Y5FLU3nC5LXgYB4aa6",
    "Fh9HmeLNUMVCvejxCtCL2DbYaRyBFVJ5xrWkLnMH6fdk",
    "463MEnMeGyJekNZFQSTUABBEbLnvMTALbT6ZmsxAbAdq",
    "6AUH3WEHucYZyC61hqpqYUWVto5qA5hjHuNQ32GNnNxA",
];
/// PumpSwap buyback fee recipients (pick any; the program accepts all 8)
pub const PUMPSWAP_BUYBACK_FEE_RECIPIENT: &str = "5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD";

// ---- On-Chain Arbitrage Router — Instruction Discriminators ----
// WARNING: these must match programs/arbitrage/src/constants.rs.
//   ROUTE_PUMP_TO_DLMM_DISC ←→ programs/arbitrage/src/constants.rs: ROUTE_PUMP_TO_DLMM_DISC
//   ROUTE_DLMM_TO_PUMP_DISC ←→ programs/arbitrage/src/constants.rs: ROUTE_DLMM_TO_PUMP_DISC
//   ROUTE_DISC              ←→ programs/arbitrage/src/constants.rs: ROUTE_DISC
// If you change a discriminator on-chain, update both files or the Router will
// silently reject all instructions with ARB_BAD_DISCRIMINATOR.
// Program ID is in config.toml → [execution_routing].onchain_program_id.
pub const ROUTE_PUMP_TO_DLMM_DISC: [u8; 8] = [0x8b, 0xe8, 0x20, 0x55, 0xc1, 0xb0, 0xc1, 0xe9];
/// sha256("global:route_dlmm_to_pump")[..8]
pub const ROUTE_DLMM_TO_PUMP_DISC: [u8; 8] = [0x17, 0x6e, 0xcc, 0x5d, 0xdd, 0x93, 0x51, 0x95];
/// sha256("global:arbitrage_route")[..8] — generic dynamic DEX router
pub const ROUTE_DISC: [u8; 8] = [0x5f, 0x0f, 0x91, 0x02, 0x9a, 0x03, 0x4c, 0xc3];

// ============================================================
// On-Chain Program Enum Mapping
// WARNING: must match programs/arbitrage/src/constants.rs.
// These are wire-format discriminants — changing them breaks the
// client↔program contract and all routes start failing with
// ARB_UNKNOWN_DEX_PAIR.
// ============================================================

// ---- DEX kind identifiers (must match on-chain DexKind enum) ----
pub const DEX_KIND_PUMPSWAP: u8 = 0;
pub const DEX_KIND_DLMM: u8 = 1;
pub const DEX_KIND_CPMM: u8 = 2;
pub const DEX_KIND_WHIRLPOOL: u8 = 3;
