//! PumpSwap AMM helpers and common utilities.

use anyhow::Context;

/// Convert a SOL amount to lamports with overflow check.
/// Uses `u64::MAX` cap rather than wrapping on overflow — prevents
/// silent truncation of large values in financial calculations.
#[inline]
pub(crate) fn sol_to_lamports(sol: f64) -> u64 {
    let lamports = sol * 1_000_000_000.0;
    if lamports < 0.0 {
        0
    } else if lamports > u64::MAX as f64 {
        u64::MAX
    } else {
        lamports as u64
    }
}
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::LazyLock;
use tokio::sync::RwLock;

use crate::simulator;
use crate::simulator::PumpSwapPoolMeta;

/// Pool metadata cache — keyed by pool Pubkey.
/// Pool vault ATAs, coin_creator, and cashback flags are immutable after pool creation,
/// so entries never expire.
static POOL_META_CACHE: LazyLock<RwLock<HashMap<Pubkey, PumpSwapPoolMeta>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Combined pool read: returns metadata + fresh reserves in one `get_account` call.
/// Caches the immutable metadata (vault ATAs, coin_creator, flags) under `POOL_META_CACHE`.
pub(crate) async fn fetch_pumpswap_meta_and_reserves(
    rpc: &RpcClient,
    pool: &Pubkey,
) -> anyhow::Result<(PumpSwapPoolMeta, u64, u64)> {
    // Check metadata cache first — vault addresses are immutable after pool creation
    let meta = {
        let cache = POOL_META_CACHE.read().await;
        cache.get(pool).cloned()
    };

    let meta = match meta {
        Some(m) => m,
        None => {
            let account = rpc
                .get_account(pool)
                .await
                .context("fetch pumpswap pool account")?;
            let m = simulator::parse_pumpswap_pool_meta(&account.data)
                .context("parse pumpswap pool metadata")?;
            let mut cache = POOL_META_CACHE.write().await;
            cache.insert(*pool, m.clone());
            m
        }
    };

    // Read both vault token accounts in parallel for fresh balances
    let sol_mint = crate::constants::NATIVE_SOL_MINT;
    let (base_vault_res, quote_vault_res) = tokio::join!(
        rpc.get_account(&meta.pool_base_token_account),
        rpc.get_account(&meta.pool_quote_token_account),
    );

    let base_data = base_vault_res
        .as_ref()
        .map(|a| &a.data)
        .map_err(|e| anyhow::anyhow!("base vault read: {e}"))?;
    let quote_data = quote_vault_res
        .map(|a| a.data)
        .map_err(|e| anyhow::anyhow!("quote vault read: {e}"))?;

    let base_amount = parse_spl_token_amount(base_data);
    let quote_amount = parse_spl_token_amount(&quote_data);
    let base_mint = read_pubkey_at(base_data, 0);
    let quote_mint = read_pubkey_at(&quote_data, 0);

    let (sol_res, tok_res) = if base_mint.to_string() == sol_mint {
        (base_amount, quote_amount)
    } else if quote_mint.to_string() == sol_mint {
        (quote_amount, base_amount)
    } else {
        anyhow::bail!(
            "PumpSwap pool {} not SOL-denominated (base={} quote={})",
            pool,
            base_mint,
            quote_mint,
        );
    };

    Ok((meta, sol_res, tok_res))
}

fn read_pubkey_at(data: &[u8], off: usize) -> Pubkey {
    Pubkey::new_from_array(data[off..off + 32].try_into().unwrap())
}

fn parse_spl_token_amount(data: &[u8]) -> u64 {
    u64::from_le_bytes(data[64..72].try_into().unwrap_or([0; 8]))
}

/// Build a system program transfer instruction
pub(crate) fn system_transfer_ix(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes()); // Transfer instruction index
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: system_program,
        accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
        data,
    }
}

/// Build a SyncNative instruction to update WSOL ATA balance to match lamports
pub(crate) fn sync_native_ix(wsol_ata: &Pubkey, token_program: &Pubkey) -> Instruction {
    Instruction {
        program_id: *token_program,
        accounts: vec![AccountMeta::new(*wsol_ata, false)],
        data: vec![17], // SyncNative
    }
}

/// Build a CloseAccount instruction to unwrap WSOL back to SOL
pub(crate) fn close_wsol_ata_ix(
    wsol_ata: &Pubkey,
    wallet: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: *token_program,
        accounts: vec![
            AccountMeta::new(*wsol_ata, false),
            AccountMeta::new(*wallet, false),          // destination
            AccountMeta::new_readonly(*wallet, false), // authority
        ],
        data: vec![9], // CloseAccount
    }
}

/// Pick a protocol fee recipient for a PumpSwap pool based on `is_mayhem_mode`.
/// Selects from the 8 well-known `PROTOCOL_FEE_RECIPIENTS` (normal mode) or
/// `RESERVED_FEE_RECIPIENTS` (Mayhem mode). The on-chain program accepts any of
/// the 8 in the appropriate list, so the client picks one arbitrarily.
pub(crate) fn pick_pumpswap_protocol_fee_recipient(is_mayhem_mode: bool) -> Pubkey {
    let list: &[&str; 8] = if is_mayhem_mode {
        &crate::constants::PUMPSWAP_RESERVED_FEE_RECIPIENTS
    } else {
        &crate::constants::PUMPSWAP_PROTOCOL_FEE_RECIPIENTS
    };
    Pubkey::from_str(list[0]).unwrap()
}
