//! On-chain transaction simulator & instruction builder
//!
//! Supports Raydium CPMM, AMMv4, Pump.fun, Meteora DLMM.
//! Provides swap instruction building and transaction simulation for cross-pool arbitrage.
//!
//! Sub-modules split by venue: ammv4 / cpmm / pumpfun / dlmm
#![allow(dead_code)] // AMMv4/CPMM builders reserved for venue expansion

mod ammv4;
mod bonding_curve;
mod cpmm;
mod dlmm;
mod pumpswap;

// Re-export venue-specific items
pub use ammv4::*;
pub use bonding_curve::*;
#[allow(unused_imports)]
pub use cpmm::*;
pub use dlmm::*;
pub use pumpswap::*;

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcSimulateTransactionConfig;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::LazyLock;
use tokio::sync::RwLock;

// Constants used by external modules via simulator:: path
pub use crate::constants::{DLMM_PROGRAM, NATIVE_SOL_MINT, TOKEN_PROGRAM};
// Constants used internally by sub-modules (via super::)
use crate::constants::{
    AMMV4_SWAP_DISCRIMINATOR, AMMV4_SWAP_OUT_DISCRIMINATOR, ATA_PROGRAM, CPMM_SWAP_DISCRIMINATOR,
    DLMM_SWAP2_DISCRIMINATOR, MEMO_PROGRAM, PUMPFUN_BONDING_CURVE_PROGRAM,
    PUMPFUN_BUY_DISCRIMINATOR, PUMPFUN_SELL_DISCRIMINATOR, PUMPSWAP_BUY_DISCRIMINATOR,
    PUMPSWAP_SELL_DISCRIMINATOR, SERUM_PROGRAM, SYSVAR_RENT, TOKEN22_PROGRAM,
};

// ============================================================
// Public utility functions
// ============================================================

fn pubkey_from_str(s: &str) -> Option<Pubkey> {
    Pubkey::from_str(s).ok()
}

/// Build CreateIdempotent ATA instruction (supports Tokenkeg / Token-2022)
pub fn create_ata_idempotent_ix_v2(
    payer: &Pubkey,
    ata: &Pubkey,
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Instruction {
    let ata_program = Pubkey::from_str(ATA_PROGRAM).unwrap();
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let sysvar_rent = Pubkey::from_str(SYSVAR_RENT).unwrap();

    Instruction {
        program_id: ata_program,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program, false),
            AccountMeta::new_readonly(*token_program, false),
            AccountMeta::new_readonly(sysvar_rent, false),
        ],
        data: vec![1], // CreateIdempotent
    }
}

/// Per-mint token program cache — mint owners are immutable, so entries never expire.
static TOKEN_PROGRAM_CACHE: LazyLock<RwLock<HashMap<String, Pubkey>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Detect which token program a mint uses (with global cache; mint owner never changes)
pub async fn detect_token_program(rpc: &RpcClient, mint: &Pubkey) -> anyhow::Result<Pubkey> {
    let key = mint.to_string();
    {
        let cache = TOKEN_PROGRAM_CACHE.read().await;
        if let Some(tp) = cache.get(&key) {
            return Ok(*tp);
        }
    }
    let account = rpc.get_account(mint).await?;
    let owner = account.owner;
    let token22 = Pubkey::from_str(TOKEN22_PROGRAM).unwrap();
    let token = Pubkey::from_str(TOKEN_PROGRAM).unwrap();
    let tp = if owner == token22 { token22 } else { token };
    let mut cache = TOKEN_PROGRAM_CACHE.write().await;
    cache.insert(key, tp);
    Ok(tp)
}

/// ATA address derivation
pub fn ata_addr(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    let ata_program = Pubkey::from_str(ATA_PROGRAM).unwrap();
    Pubkey::find_program_address(
        &[
            &wallet.to_bytes(),
            &token_program.to_bytes(),
            &mint.to_bytes(),
        ],
        &ata_program,
    )
    .0
}

/// Constant-product AMM output estimation (matches pump-rust-client additive-fee formula)
pub fn estimate_swap_output(
    input_vault_raw: u64,
    output_vault_raw: u64,
    amount_in: u64,
    fee: f64,
) -> u64 {
    let ri = input_vault_raw as u128;
    let ro = output_vault_raw as u128;
    let a = amount_in as u128;
    let fee_bps = (fee * 10000.0) as u128;
    let a_after_fee = a * 10000 / (10000 + fee_bps);

    let denom = ri + a_after_fee;
    if denom == 0 {
        return 0;
    }
    let out = ro * a_after_fee / denom;
    out as u64
}

/// Checked constant-product swap estimation (u128, overflow-safe).
///
/// Returns `None` when the intermediate product would overflow u128, or when
/// the output rounds to 0 — both conditions would cause the on-chain
/// PumpSwap program to reject the swap.
///
/// Formula matches pump-rust-client v0.1.3 `math/amm.rs`:
///   denom = 10000 + fee_bps
///   effective = amount_in * 10000 / denom
///   output = reserve_out * effective / (reserve_in + effective)
///
/// Fee is ADDITIVE in the denominator (not subtractive from the numerator).
pub fn checked_estimate_swap_output(
    input_vault_raw: u64,
    output_vault_raw: u64,
    amount_in: u64,
    fee_bps: u32,
) -> Option<u64> {
    let ri = input_vault_raw as u128;
    let ro = output_vault_raw as u128;
    let a = amount_in as u128;
    if a == 0 {
        return Some(0);
    }
    // Step 1: effective_input = amount_in * 10000 / (10000 + fee_bps)
    let denom = 10_000u128.checked_add(fee_bps as u128)?;
    let num = a.checked_mul(10_000u128)?;
    let effective = num.checked_div(denom)?;
    if effective == 0 {
        return None;
    }
    // Step 2: output = reserve_out * effective / (reserve_in + effective)
    let numerator = ro.checked_mul(effective)?;
    let denominator = ri.checked_add(effective)?;
    if denominator == 0 {
        return None;
    }
    let out = numerator.checked_div(denominator)?;
    if out == 0 {
        return None;
    }
    Some(out as u64)
}

/// Simulate a single serialized transaction (deserialize → simulate via RPC)
pub(crate) async fn simulate_serialized_tx(rpc: &RpcClient, tx_bytes: &[u8]) -> anyhow::Result<()> {
    use solana_sdk::commitment_config::CommitmentConfig;
    use solana_sdk::transaction::{Transaction, VersionedTransaction};

    let tx: VersionedTransaction = bincode::deserialize(tx_bytes)
        .or_else(|_| {
            bincode::deserialize::<Transaction>(tx_bytes).map(|legacy| legacy.into())
        })
        .map_err(|e| anyhow::anyhow!("deserialize tx for simulation: {e}"))?;

    let config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(CommitmentConfig::processed()),
        encoding: None,
        accounts: None,
        min_context_slot: None,
        inner_instructions: false,
    };

    let response = rpc
        .simulate_transaction_with_config(&tx, config)
        .await
        .map_err(|e| anyhow::anyhow!("simulate RPC error: {e}"))?;

    if let Some(err) = response.value.err {
        anyhow::bail!(
            "simulation failed: {} (logs: {})",
            err,
            response.value.logs.unwrap_or_default().join("; ")
        );
    }
    Ok(())
}

/// Diagnostic: re-simulate a failed TX with zero slippage (min_amount_out = 0)
/// to isolate whether failures are caused by formula overestimation or data staleness.
///
/// - If zero-slippage sim PASSES → the original failure was formula overestimation
///   (the estimate claimed more output than the actual pool state can deliver).
/// - If zero-slippage sim STILL FAILS → data inconsistency (pool state changed
///   between TX build and sim, or wrong accounts).
///
/// Returns a human-readable diagnostic string for logging.
pub(crate) async fn diagnostic_zero_slippage_sim(
    rpc: &RpcClient,
    tx_bytes: &[u8],
    onchain_program_id: &Pubkey,
) -> anyhow::Result<String> {
    use solana_sdk::commitment_config::CommitmentConfig;
    use solana_sdk::message::VersionedMessage;
    use solana_sdk::pubkey::Pubkey;
    use solana_sdk::transaction::{Transaction, VersionedTransaction};
    use std::str::FromStr;
    use crate::constants;

    // Deserialize
    let mut tx: VersionedTransaction = bincode::deserialize(tx_bytes)
        .or_else(|_| {
            bincode::deserialize::<Transaction>(tx_bytes).map(|legacy| legacy.into())
        })
        .map_err(|e| anyhow::anyhow!("diagnostic deserialize: {e}"))?;

    let dlmm_id = Pubkey::from_str(constants::DLMM_PROGRAM).unwrap();
    let pumpswap_id = Pubkey::from_str(constants::PUMPFUN_AMM_PROGRAM).unwrap();
    let bonding_id = Pubkey::from_str(constants::PUMPFUN_BONDING_CURVE_PROGRAM).unwrap();
    let onchain_arb_id = *onchain_program_id;

    let mut modified = 0u8;

    let zero_min_out_direct = |data: &mut Vec<u8>| {
        // PumpSwap/DLMM/Bonding: min_amount_out at offset 16 (8 bytes)
        if data.len() >= 24 {
            data[16..24].fill(0);
            true
        } else {
            false
        }
    };

    let zero_min_out_onchain = |data: &mut Vec<u8>| {
        // On-chain program ROUTE_DISC / legacy: min_amount_out as last 8 bytes
        if data.len() >= 8 {
            let n = data.len();
            data[n - 8..n].fill(0);
            true
        } else {
            false
        }
    };

    match &mut tx.message {
        VersionedMessage::Legacy(msg) => {
            for ix in &mut msg.instructions {
                let pid = msg.account_keys[ix.program_id_index as usize];
                let zeroed = if pid == onchain_arb_id {
                    zero_min_out_onchain(&mut ix.data)
                } else if pid == dlmm_id || pid == pumpswap_id || pid == bonding_id {
                    zero_min_out_direct(&mut ix.data)
                } else {
                    false
                };
                if zeroed {
                    modified += 1;
                }
            }
        }
        VersionedMessage::V0(msg) => {
            for ix in &mut msg.instructions {
                let pid = msg.account_keys[ix.program_id_index as usize];
                let zeroed = if pid == onchain_arb_id {
                    zero_min_out_onchain(&mut ix.data)
                } else if pid == dlmm_id || pid == pumpswap_id || pid == bonding_id {
                    zero_min_out_direct(&mut ix.data)
                } else {
                    false
                };
                if zeroed {
                    modified += 1;
                }
            }
        }
    }

    if modified == 0 {
        return Err(anyhow::anyhow!(
            "diagnostic: no swap ix found in TX (no known program matched)"
        ));
    }

    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(CommitmentConfig::processed()),
        encoding: None,
        accounts: None,
        min_context_slot: None,
        inner_instructions: false,
    };

    let response = rpc
        .simulate_transaction_with_config(&tx, sim_config)
        .await
        .map_err(|e| anyhow::anyhow!("diagnostic sim RPC error: {e}"))?;

    if let Some(err) = response.value.err {
        anyhow::bail!(
            "DIAGNOSTIC: zero-slippage sim STILL FAILED → DATA INCONSISTENCY (not formula). \
             error={} logs={}",
            err,
            response.value.logs.unwrap_or_default().join("; ")
        );
    }

    let logs = response.value.logs.unwrap_or_default();
    let cu = logs
        .iter()
        .rev()
        .find(|l| l.contains("consumed"))
        .map(|l| l.as_str())
        .unwrap_or("? CU");
    Ok(format!(
        "DIAGNOSTIC: zero-slippage sim PASSED → FORMULA OVERESTIMATION. \
         zeroed={} ix, {}",
        modified, cu
    ))
}

#[cfg(test)]
mod tests {
    use super::estimate_swap_output;

    #[test]
    fn estimate_swap_output_extremes() {
        assert_eq!(estimate_swap_output(1000, 1000, 0, 0.0), 0);
        let out = estimate_swap_output(1000, 1000, 100, 0.0);
        assert!(out > 0);
        let out = estimate_swap_output(1000, 1000, 1_000_000, 0.0);
        assert!(out < 1000, "can't extract more than reserves");
    }
}
