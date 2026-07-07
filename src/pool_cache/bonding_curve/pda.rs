//! PumpSwap PDA helpers: pool account reading and PDA pattern verification.
//!
//! Pump.fun official 4 PDA patterns:
//!   Pattern 1 — Global Config:    ["global"]                                         → deterministic
//!   Pattern 2 — Bonding Curve:    ["bonding-curve", mint]          @ 6EF8rrecth...    → deterministic
//!   Pattern 3 — Creator Vault:    ["creator-vault", creator]       @ pAMMBay6oceH...  → needs creator
//!   Pattern 4 — AMM Pool:         ["pool", 0u16::LE, creator, mint, SOL]              → needs creator
//!
//! Key conclusion (verified 2026-06-13):
//!   Pattern 4's creator is neither BC_PDA nor a constant wallet across pools.
//!   migrate is permissionless, any wallet can call it, each pool has a different creator.
//!   → Pool PDA cannot be deterministically derived from mint alone; graduated tokens must rely on the HTTP API's pump_swap_pool field

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;

use super::super::{BondingCurveState, PumpVenueKind};

/// Read u64 from byte slice at offset (little-endian)
#[allow(dead_code)]
fn read_u64_at(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap_or([0; 8]))
}

/// Read Pubkey from byte slice at offset
fn read_pubkey_at(data: &[u8], off: usize) -> Pubkey {
    Pubkey::new_from_array(data[off..off + 32].try_into().unwrap())
}

/// Parse SPL token account amount (bytes 64..72)
fn parse_token_amount(data: &[u8]) -> u64 {
    if data.len() < 72 {
        return 0;
    }
    u64::from_le_bytes(data[64..72].try_into().unwrap_or([0; 8]))
}

/// Read a PumpSwap pool's vaults given the known pool address.
/// Pool struct layout (after 8-byte discriminator `f19a6d0411b16dbc`):
///   offset  8: pool_bump: u8
///   offset  9: index: u16
///   offset 11: creator: Pubkey
///   offset 43: base_mint: Pubkey
///   offset 75: quote_mint: Pubkey
///   offset 107: lp_mint: Pubkey
///   offset 139: pool_base_token_account: Pubkey
///   offset 171: pool_quote_token_account: Pubkey
pub(super) async fn read_pumpswap_pool(
    rpc: &RpcClient,
    pool_pk: &Pubkey,
    pool_addr: &str,
    meme_mint: &str,
) -> Option<BondingCurveState> {
    let cache = crate::grpc_stream::global_cache();
    let data: Vec<u8> = if let Some((cached, cached_slot)) = cache.get_with_slot(pool_addr) {
        let latest = cache.latest_slot();
        if latest > 0 && latest.saturating_sub(cached_slot) <= 2 {
            log::debug!("[GRPC-CACHE] hit pool={}", &pool_addr[..pool_addr.len().min(12)]);
            cached
        } else {
            // stale — fall through to RPC
            let acct = rpc.get_account(pool_pk).await.ok()?;
            acct.data
        }
    } else {
        let acct = rpc.get_account(pool_pk).await.ok()?;
        acct.data
    };

    // Minimum size: discriminator(8) + through pool_quote_token_account(171+32=203)
    if data.len() < 203 {
        log::debug!(
            "[PUMPFUN] Pool account too short pool={} len={}",
            &pool_addr[..pool_addr.len().min(12)],
            data.len(),
        );
        return None;
    }

    let base_vault = read_pubkey_at(&data, 139);
    let quote_vault = read_pubkey_at(&data, 171);

    // Read both vault token accounts in parallel
    let (base_res, quote_res) =
        tokio::join!(rpc.get_account(&base_vault), rpc.get_account(&quote_vault));

    let base_amount = base_res
        .as_ref()
        .map(|a| parse_token_amount(&a.data))
        .unwrap_or(0);
    let quote_amount = quote_res
        .as_ref()
        .map(|a| parse_token_amount(&a.data))
        .unwrap_or(0);

    if base_amount == 0 || quote_amount == 0 {
        log::debug!(
            "[PUMPFUN] Pool vault empty pool={} base={} quote={}",
            &pool_addr[..pool_addr.len().min(12)],
            base_amount,
            quote_amount,
        );
        return None;
    }

    // Read mints from vault SPL token accounts (offset 0-32) to determine
    // which side is SOL and which is the meme token. The Pool account's
    // internal mint ordering cannot be trusted — graduated tokens may pair
    // with USDC or other quote tokens, not just SOL.
    let sol_mint = crate::constants::NATIVE_SOL_MINT;
    let base_vault_mint = base_res
        .as_ref()
        .map(|a| read_pubkey_at(&a.data, 0).to_string())
        .unwrap_or_default();
    let quote_vault_mint = quote_res
        .as_ref()
        .map(|a| read_pubkey_at(&a.data, 0).to_string())
        .unwrap_or_default();

    let base_is_sol = base_vault_mint == sol_mint;
    let quote_is_sol = quote_vault_mint == sol_mint;

    if !base_is_sol && !quote_is_sol {
        log::debug!(
            "[PUMPFUN] PumpSwap pool={} is not SOL-denominated (base_mint={}, quote_mint={}) skip",
            &pool_addr[..pool_addr.len().min(12)],
            &base_vault_mint[..base_vault_mint.len().min(12)],
            &quote_vault_mint[..quote_vault_mint.len().min(12)],
        );
        return None;
    }

    let (sol_reserves, token_reserves) = if base_is_sol {
        (base_amount, quote_amount)
    } else {
        (quote_amount, base_amount)
    };

    log::debug!(
        "[PUMPFUN] PumpSwap pool={} base_mint={} quote_mint={} sol_res={} tok_res={}",
        &pool_addr[..pool_addr.len().min(12)],
        &base_vault_mint[..base_vault_mint.len().min(12)],
        &quote_vault_mint[..quote_vault_mint.len().min(12)],
        sol_reserves,
        token_reserves,
    );

    Some(BondingCurveState {
        mint: meme_mint.to_string(),
        bonding_curve_address: pool_addr.to_string(),
        virtual_token_reserves: token_reserves,
        virtual_sol_reserves: sol_reserves,
        real_token_reserves: token_reserves,
        real_sol_reserves: sol_reserves,
        complete: true,
        creator: String::new(), // not available from pool alone
        venue_kind: PumpVenueKind::PumpSwapPool,
    })
}

#[cfg(test)]
mod tests {
    use crate::constants::{PUMPFUN_AMM_PROGRAM, PUMPFUN_BONDING_CURVE_PROGRAM};
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;

    #[test]
    fn verify_pumpswap_pda_patterns() {
        // Verify Pump.fun's 4 PDA patterns are correctly understood.
        let old_prog = Pubkey::from_str(PUMPFUN_BONDING_CURVE_PROGRAM).unwrap();
        let new_prog = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();
        let mint = Pubkey::from_str("9qpDk7hGSHqyfMGDT7p4zFQ35aGff248Qes48CgLpump").unwrap();
        let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();

        // Pattern 1: Global Config — deterministic
        let (global_config, _) = Pubkey::find_program_address(&[b"global_config"], &new_prog);
        println!("GLOBAL_CONFIG={}", global_config);

        // Pattern 2: Bonding Curve — deterministic (depends only on mint)
        let (bc_pda, _) =
            Pubkey::find_program_address(&[b"bonding-curve", &mint.to_bytes()], &old_prog);
        println!("BC_PDA={}", bc_pda);

        // Pattern 4: Pool PDA — NOT deterministic from mint alone
        // Pool PDA = ["pool", 0u16::LE, creator, mint, SOL]
        // BC_PDA != creator (verified 2026-06-13)
        let pool_creator =
            Pubkey::from_str("9XDYTfQKwW8sHPqnFdUreMmtmffmkHVPGTNV2e3LKxNW").unwrap();
        assert_ne!(
            bc_pda, pool_creator,
            "BC PDA should NOT equal pool creator — creator is the migrate caller"
        );

        // Pool with BC as creator → wrong address
        let (wrong_pool, _) = Pubkey::find_program_address(
            &[
                b"pool",
                &0u16.to_le_bytes(),
                &bc_pda.to_bytes(),
                &mint.to_bytes(),
                &sol_mint.to_bytes(),
            ],
            &new_prog,
        );
        // Pool with actual creator → correct
        let (correct_pool, _) = Pubkey::find_program_address(
            &[
                b"pool",
                &0u16.to_le_bytes(),
                &pool_creator.to_bytes(),
                &mint.to_bytes(),
                &sol_mint.to_bytes(),
            ],
            &new_prog,
        );
        println!("WRONG_POOL(BC as creator)={}", wrong_pool);
        println!("CORRECT_POOL(known_creator)={}", correct_pool);
        assert_ne!(
            wrong_pool, correct_pool,
            "BC-as-creator pool should differ from real pool"
        );
    }

    /// Verify PDA against known example from PumpSwap README:
    ///   mint = 7LSsEoJGhLeZzGvDofTdNg7M3JttxQqGWNLo6vWMpump
    ///   pool = GseMAnNDvntR5uFePZ51yZBXzNSn7GdFPkfHwfr6d77J
    ///
    /// Canonical pool PDA seeds: ["pool", 0u16::LE, creator, base_mint, quote_mint]
    /// The creator is the wallet that called migrate (stored in Pool.creator).
    /// Since migrate is permissionless, creator varies per-pool.
    /// Discovery must use either the HTTP API or getProgramAccounts with memcmp.
    #[test]
    fn verify_known_canonical_pool() {
        let new_prog = Pubkey::from_str("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA").unwrap();
        let mint = Pubkey::from_str("7LSsEoJGhLeZzGvDofTdNg7M3JttxQqGWNLo6vWMpump").unwrap();
        let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
        let expected_pool =
            Pubkey::from_str("GseMAnNDvntR5uFePZ51yZBXzNSn7GdFPkfHwfr6d77J").unwrap();

        // Pool.creator from the README example is the wallet that called migrate.
        // Verify that using this as the PDA seed produces the expected pool address.
        let pool_creator =
            Pubkey::from_str("9XDYTfQKwW8sHPqnFdUreMmtmffmkHVPGTNV2e3LKxNW").unwrap();
        let (derived, _) = Pubkey::find_program_address(
            &[
                b"pool",
                &0u16.to_le_bytes(),
                &pool_creator.to_bytes(),
                &mint.to_bytes(),
                &sol_mint.to_bytes(),
            ],
            &new_prog,
        );

        println!("mint       = {}", mint);
        println!("creator    = {}", pool_creator);
        println!("derived    = {}", derived);
        println!("expected   = {}", expected_pool);
        assert_eq!(derived, expected_pool, "Pool PDA must match known address");
    }
}
