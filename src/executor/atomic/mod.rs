//! Single atomic TX building & submission — replaces Jito bundle
//!
//! Packs PumpSwap buy + DLMM sell (or reverse) into a single Solana transaction,
//! leveraging Solana transaction atomicity: all-or-nothing execution.
//!
//! Reuses instruction builders from src/simulator/ and pool queries from src/pool_cache/.

mod builders_cpmm_wp;
mod builders_legacy;
mod builders_pump_dlmm;
mod dlmm_amm_to_pump;
mod dlmm_bonding_to_pump;
mod generic_route;
mod helpers;
mod onchain_router;
pub(crate) use onchain_router::warmup_tp_cache;
mod pump_amm_to_dlmm;
mod pump_bonding_to_dlmm;

use anyhow::Context;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_config::RpcSendTransactionConfig;
use solana_sdk::commitment_config::{CommitmentConfig, CommitmentLevel};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::{Transaction, VersionedTransaction};
use std::str::FromStr;
use std::sync::LazyLock;

use crate::arbitrage::{ArbitrageOpportunity, Venue};
use crate::config::AppConfig;
use crate::pool_cache;
use crate::simulator;

use dlmm_bonding_to_pump::build_dlmm_buy_pumpswap_sell;
use pump_bonding_to_dlmm::build_pumpswap_buy_dlmm_sell;

/// Build atomic arbitrage TX: buy venue → sell venue.
/// Estimate data passed from builder to confirmation for PnL comparison.
pub struct TxEstimate {
    pub invest_sol: f64,
    pub est_meme: u64,
    pub est_sol_out: u64,
}

pub async fn build_atomic_arbitrage_tx(
    opp: &ArbitrageOpportunity,
    wallet: &Keypair,
    config: &AppConfig,
    rpc: &RpcClient,
) -> anyhow::Result<(Vec<u8>, u64, Option<TxEstimate>)> {
    let wallet_pubkey = wallet.pubkey();

    // Try gRPC blockhash cache first (zero-RTT); fall back to RPC.
    let (blockhash, last_valid_block_height) = {
        let cache = crate::grpc_stream::global_cache();
        if let Some(bh) = cache.get_fresh_blockhash() {
            let slot = cache.latest_slot();
            log::debug!("[BLOCKHASH] gRPC cache hit, slot={slot}");
            (bh, slot.saturating_add(150))
        } else {
            log::debug!("[BLOCKHASH] gRPC cache miss, falling back to RPC");
            rpc.get_latest_blockhash_with_commitment(CommitmentConfig::processed())
                .await
                .context("get_latest_blockhash_with_commitment")?
        }
    };

    let sol_mint = Pubkey::from_str(simulator::NATIVE_SOL_MINT)?;
    let meme_mint = Pubkey::from_str(&opp.token_mint)?;
    let token_program = simulator::detect_token_program(rpc, &meme_mint).await?;
    let sol_token_program = Pubkey::from_str(simulator::TOKEN_PROGRAM)?;

    let user_sol_ata = simulator::ata_addr(&wallet_pubkey, &sol_mint, &sol_token_program);
    let user_meme_ata = simulator::ata_addr(&wallet_pubkey, &meme_mint, &token_program);

    let investment_lamports = (opp.investment_sol * 1_000_000_000.0) as u64;

    let (tx_bytes, est_meme, est_sol_out) = match (opp.buy_venue, opp.sell_venue) {
        (Venue::PumpSwapAmm, Venue::MeteoraDlmm) => {
            if should_use_onchain_program(&opp.token_mint, config) {
                let (tx, meme, sol) = builders_legacy::build_onchain_pump_to_dlmm_tx(
                    opp, &wallet_pubkey, wallet, config, rpc,
                    &sol_mint, &meme_mint, &token_program, &sol_token_program,
                    &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
                ).await?;
                (tx, meme, sol)
            } else {
                let tx = build_pumpswap_buy_dlmm_sell(
                    opp, &wallet_pubkey, wallet, config, rpc,
                    &sol_mint, &meme_mint, &token_program, &sol_token_program,
                    &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
                ).await?;
                (tx, 0u64, 0u64)
            }
        }
        (Venue::MeteoraDlmm, Venue::PumpSwapAmm) => {
            if should_use_onchain_program(&opp.token_mint, config) {
                let (tx, meme, sol) = builders_legacy::build_onchain_dlmm_to_pump_tx(
                    opp, &wallet_pubkey, wallet, config, rpc,
                    &sol_mint, &meme_mint, &token_program, &sol_token_program,
                    &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
                ).await?;
                (tx, meme, sol)
            } else {
                let tx = build_dlmm_buy_pumpswap_sell(
                    opp, &wallet_pubkey, wallet, config, rpc,
                    &sol_mint, &meme_mint, &token_program, &sol_token_program,
                    &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
                ).await?;
                (tx, 0u64, 0u64)
            }
        }
        (Venue::RaydiumCpmm, Venue::OrcaWhirlpool) => {
            let (tx, meme, sol) = builders_cpmm_wp::build_onchain_cpmm_to_whirlpool_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::OrcaWhirlpool, Venue::RaydiumCpmm) => {
            let (tx, meme, sol) = builders_cpmm_wp::build_onchain_whirlpool_to_cpmm_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::PumpSwapAmm, Venue::RaydiumCpmm) => {
            let (tx, meme, sol) = builders_cpmm_wp::build_onchain_pump_to_cpmm_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::RaydiumCpmm, Venue::PumpSwapAmm) => {
            let (tx, meme, sol) = builders_cpmm_wp::build_onchain_cpmm_to_pump_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::MeteoraDlmm, Venue::OrcaWhirlpool) => {
            let (tx, meme, sol) = builders_pump_dlmm::build_onchain_dlmm_to_whirlpool_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::OrcaWhirlpool, Venue::MeteoraDlmm) => {
            let (tx, meme, sol) = builders_pump_dlmm::build_onchain_whirlpool_to_dlmm_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::PumpSwapAmm, Venue::OrcaWhirlpool) => {
            let (tx, meme, sol) = builders_pump_dlmm::build_onchain_pump_to_whirlpool_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::OrcaWhirlpool, Venue::PumpSwapAmm) => {
            let (tx, meme, sol) = builders_pump_dlmm::build_onchain_whirlpool_to_pump_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::RaydiumCpmm, Venue::MeteoraDlmm) => {
            let (tx, meme, sol) = builders_pump_dlmm::build_onchain_cpmm_to_dlmm_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        (Venue::MeteoraDlmm, Venue::RaydiumCpmm) => {
            let (tx, meme, sol) = builders_pump_dlmm::build_onchain_dlmm_to_cpmm_tx(
                opp, &wallet_pubkey, wallet, config, rpc,
                &sol_mint, &meme_mint, &token_program, &sol_token_program,
                &user_sol_ata, &user_meme_ata, investment_lamports, &blockhash,
            ).await?;
            (tx, meme, sol)
        }
        _ => anyhow::bail!(
            "unsupported venue pair for atomic TX: {:?} → {:?}",
            opp.buy_venue, opp.sell_venue,
        ),
    };

    // Inject Jito tip at build time — before the message is compiled.
    // Uses native Instruction (not clone-in-place), guaranteeing correctness.
    let tx_bytes = if config.solana.sender_enabled {
        inject_jito_tip(&tx_bytes, wallet, config, rpc).await?
    } else {
        tx_bytes
    };

    let estimate = if est_meme > 0 || est_sol_out > 0 {
        Some(TxEstimate { invest_sol: opp.investment_sol, est_meme, est_sol_out })
    } else {
        None
    };
    Ok((tx_bytes, last_valid_block_height, estimate))
}

// ============================================================
// Jito tip injection (build-phase, native Instruction)
// ============================================================

/// Jito tip accounts (mainnet).
const JITO_TIP_ACCOUNTS: &[&str] = &[
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
];
const JITO_TIP_LAMPORTS: u64 = 100_000; // 0.0001 SOL

/// Inject a Jito tip into an already-built v0 transaction at build time.
///
/// Uses native `Instruction` objects and `try_compile` — no clone-in-place hacks.
/// Fetches real ALT data so recompilation preserves all accounts.
async fn inject_jito_tip(
    tx_bytes: &[u8],
    wallet: &Keypair,
    config: &AppConfig,
    rpc: &RpcClient,
) -> anyhow::Result<Vec<u8>> {
    use solana_sdk::{
        instruction::Instruction,
        message::{v0, VersionedMessage},
        system_instruction,
        transaction::VersionedTransaction,
    };
    use std::str::FromStr;

    let tx: VersionedTransaction = bincode::deserialize(tx_bytes)
        .context("deserialize tx for tip injection")?;

    let original = match &tx.message {
        VersionedMessage::V0(m) => m.clone(),
        _ => anyhow::bail!("only v0 messages supported"),
    };

    // Pick a tip account deterministically.
    let tip_idx = (original.recent_blockhash.as_ref()[0] as usize) % JITO_TIP_ACCOUNTS.len();
    let tip_account = Pubkey::from_str(JITO_TIP_ACCOUNTS[tip_idx])?;

    // Decompile compiled instructions → high-level Instruction objects.
    // Include ALT-resolved accounts by building the full key list.
    let alt_accounts = crate::executor::sender::get_alt_accounts_sync(
        &original.address_table_lookups,
        rpc,
    )
    .await;

    let mut all_keys = original.account_keys.clone();
    for alt in &alt_accounts {
        all_keys.extend_from_slice(&alt.addresses);
    }

    let mut instructions: Vec<Instruction> = original
        .instructions
        .iter()
        .map(|ci| {
            let program_id = all_keys[ci.program_id_index as usize];
            let accounts: Vec<_> = ci
                .accounts
                .iter()
                .map(|&idx| solana_sdk::instruction::AccountMeta {
                    pubkey: all_keys[idx as usize],
                    is_signer: false,
                    is_writable: true,
                })
                .collect();
            Instruction {
                program_id,
                accounts,
                data: ci.data.clone(),
            }
        })
        .collect();

    // Append tip transfer as a native Instruction.
    instructions.push(system_instruction::transfer(
        &wallet.pubkey(),
        &tip_account,
        JITO_TIP_LAMPORTS,
    ));

    log::debug!(
        "[JITO-TIP] injecting {} lamports to {} ({} instructions total)",
        JITO_TIP_LAMPORTS,
        &tip_account.to_string()[..8],
        instructions.len(),
    );

    let new_v0 = v0::Message::try_compile(
        &wallet.pubkey(),
        &instructions,
        &alt_accounts,
        original.recent_blockhash,
    )
    .context("recompile with tip")?;

    let new_tx = VersionedTransaction::try_new(VersionedMessage::V0(new_v0), &[wallet])
        .context("re-sign with tip")?;

    Ok(bincode::serialize(&new_tx).context("serialize tipped tx")?)
}

// ============================================================
// Shared helpers
// ============================================================

/// Compute min-out ratio: `1.0 - max_slippage`.
///
/// Dynamically scales slippage tolerance based on trade size relative to pool:
///   - Small trades (<0.1% pool): base tolerance (e.g. 2%)
///   - Medium (0.1–1%): tolerance × 1.5
///   - Large (>1%): tolerance × 2.5 (protect against high impact)
///
/// Prevents ARB_NEGATIVE_NET on large trades that move the pool significantly
/// while keeping small trades competitive with tight slippage.
fn compute_effective_slippage(amount_in: u64, reserve_in: u64, base_tolerance_bps: u32) -> f64 {
    let base = base_tolerance_bps as f64 / 10000.0;
    if reserve_in == 0 {
        return 1.0 - base;
    }
    let impact_ratio = amount_in as f64 / reserve_in as f64;
    let multiplier = if impact_ratio > 0.01 {
        2.5
    } else if impact_ratio > 0.001 {
        1.5
    } else {
        1.0
    };
    1.0 - (base * multiplier)
}

/// Total CU cost in SOL: priority fee + base transaction fee.
///
/// `micro_lamports × cu_limit / 1_000_000 / 1_000_000_000 + 0.000_005`
/// where 0.000_005 SOL = 5000 lamports base tx fee.
pub(crate) fn compute_cu_cost_sol(scanner: &crate::config::ScannerConfig) -> f64 {
    scanner.compute_unit_price_micro_lamports as f64
        * scanner.compute_unit_limit as f64
        / 1_000_000.0 // micro-lamports → lamports
        / 1_000_000_000.0 // lamports → SOL
        + 0.000_005 // base tx fee ~5000 lamports
}

/// Hash-based canary for on-chain program traffic routing.
///
/// Deterministic: `(token_mint.as_bytes()[0] as u64) % 100 < onchain_traffic_pct`
/// routes the trade to the on-chain program; otherwise falls back to legacy.
fn should_use_onchain_program(token_mint: &str, config: &AppConfig) -> bool {
    if !config.execution_routing.use_onchain_program {
        return false;
    }
    if config.execution_routing.onchain_arb_alt.is_none() {
        log::warn!("onchain_arb_alt not configured — falling back to legacy TX builders");
        return false;
    }
    let pct = config.execution_routing.onchain_traffic_pct;
    if pct == 0 {
        return false;
    }
    if pct >= 100 {
        return true;
    }
    let byte = token_mint.as_bytes().first().copied().unwrap_or(0);
    (byte as u64) % 100 < pct as u64
}

/// Fee recipient is global config, cached after first lookup.
static FEE_RECIPIENT_CACHE: LazyLock<std::sync::Mutex<Option<Pubkey>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

async fn resolve_pumpfun_fee_recipient(rpc: &RpcClient, config: &AppConfig) -> Pubkey {
    {
        let cache = FEE_RECIPIENT_CACHE.lock().unwrap();
        if let Some(pk) = *cache {
            return pk;
        }
    }
    let addr_str = pool_cache::fetch_pumpfun_fee_recipient(rpc)
        .await
        .unwrap_or_else(|| config.arbitrage.pumpfun_fee_recipient.clone());
    let pk = Pubkey::from_str(&addr_str).unwrap_or_else(|_| {
        Pubkey::from_str("CebN5WGQ4jvEPvsVU4EoHEpgT1mKQ7AFUbxmAhvFUWrQ").unwrap()
    });
    let mut cache = FEE_RECIPIENT_CACHE.lock().unwrap();
    *cache = Some(pk);
    pk
}

/// Submit a serialized transaction to RPC (direct submission, not via Jito)
///
/// `min_slot` sets `min_context_slot` — the RPC will process the TX only
/// once its observed slot is at least `min_slot`, preventing execution on
/// a fork behind the price-snapshot slot.
///
/// `last_valid_block_height` is the blockhash expiry height; we skip
/// submission if the current block height is within 10 blocks of expiry.
pub async fn submit_atomic_tx(
    rpc: &RpcClient,
    tx_bytes: &[u8],
    min_slot: u64,
    last_valid_block_height: u64,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        min_slot > 0,
        "min_slot must be > 0; got 0 (unset snapshot slot)"
    );

    let tx: VersionedTransaction = bincode::deserialize(tx_bytes)
        .or_else(|_| {
            bincode::deserialize::<Transaction>(tx_bytes).map(|legacy| legacy.into())
        })
        .context("deserialize tx for submission")?;

    let config = RpcSendTransactionConfig {
        skip_preflight: true,
        preflight_commitment: Some(CommitmentLevel::Processed),
        encoding: None,
        max_retries: Some(1),
        min_context_slot: Some(min_slot),
    };

    let sig = rpc
        .send_transaction_with_config(&tx, config)
        .await
        .map_err(|e| anyhow::anyhow!("sendTransaction failed: {e}"))?;

    Ok(sig.to_string())
}

/// Pre-submission simulation
pub async fn simulate_atomic_tx(rpc: &RpcClient, tx_bytes: &[u8]) -> anyhow::Result<()> {
    simulator::simulate_serialized_tx(rpc, tx_bytes).await
}

#[cfg(test)]
mod tests {
    use super::compute_cu_cost_sol;
    use crate::config::ScannerConfig;

    #[test]
    fn compute_cu_cost_sol_formula() {
        // 10_000 microLamports × 600_000 CU = 0.000006 SOL priority + 0.000005 base = 0.000011 SOL
        let scanner = ScannerConfig {
            compute_unit_price_micro_lamports: 10_000,
            compute_unit_limit: 600_000,
            ..Default::default()
        };
        let cu = compute_cu_cost_sol(&scanner);
        assert!(
            (cu - 0.000011).abs() < 1e-9,
            "expected 0.000011, got {}",
            cu
        );
    }
}
