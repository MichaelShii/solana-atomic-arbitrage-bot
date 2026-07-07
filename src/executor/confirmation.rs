//! Background trade confirmation & actual PnL accounting (H-02, H-03)
//!
//! After submitting a transaction, the main loop is not blocked; a dedicated
//! background task polls confirmation status. Once confirmed, the real SOL
//! change is extracted from on-chain pre/post balances to correct RiskTracker PnL.
//!
//! Flow: submit → record_submitted → confirmation task → record_confirmation

use log::{info, warn};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::executor::RpcPool;

use crate::config::RiskConfig;
use crate::metrics::SharedMetrics;
use crate::risk::SharedRiskTracker;

/// A transaction awaiting on-chain confirmation
pub struct PendingConfirmation {
    pub signature: String,
    pub wallet_pubkey: Pubkey,
    /// Estimated net profit, used as fallback when get_transaction fails
    pub estimated_net_profit_sol: f64,
    /// Expected priority fee (SOL), used for failure cost fallback
    pub priority_fee_sol: f64,
    /// Slot at which the tx was submitted (R2-M02)
    pub submitted_slot: u64,
    /// Instant when the tx was submitted (R2-M02)
    pub submitted_at: Instant,
    // ── Estimate vs. actual comparison (est_sol_out - invest = est_gross_profit) ──
    pub invest_sol: f64,
    pub est_meme: u64,
    pub est_sol_out: u64,
    pub route: String, // "pump→dlmm" or "dlmm→pump"
}

/// Start background confirmation task, returns sender for submitting pending confirmations
pub fn spawn_confirmation_task(
    rpc_pool: Arc<RpcPool>,
    risk_tracker: SharedRiskTracker,
    risk_config: Arc<RiskConfig>,
    metrics: SharedMetrics,
) -> mpsc::UnboundedSender<PendingConfirmation> {
    let (tx, mut rx) = mpsc::unbounded_channel::<PendingConfirmation>();

    tokio::spawn(async move {
        let mut pending: Vec<PendingConfirmation> = Vec::with_capacity(32);
        let mut interval = tokio::time::interval(Duration::from_secs(3));

        loop {
            // Collect new submissions
            while let Ok(p) = rx.try_recv() {
                pending.push(p);
            }

            if !pending.is_empty() {
                let signatures: Vec<Signature> = pending
                    .iter()
                    .filter_map(|p| Signature::from_str(&p.signature).ok())
                    .collect();

                if !signatures.is_empty() {
                    let rpc = rpc_pool.current();
                    match rpc.get_signature_statuses(&signatures).await {
                        Ok(response) => {
                            process_statuses(
                                &rpc,
                                &mut pending,
                                &signatures,
                                &response.value,
                                &risk_tracker,
                                &risk_config,
                                &metrics,
                            )
                            .await;
                        }
                        Err(e) => {
                            warn!("[CONFIRM] get_signature_statuses error: {e}");
                        }
                    }
                }

                // Evict stale entries (>128 pending → drop the older half)
                if pending.len() > 128 {
                    let drain = pending.len() - 64;
                    warn!("[CONFIRM] evicting {} stale confirmations", drain);
                    pending.drain(0..drain);
                }
            }

            interval.tick().await;
        }
    });

    tx
}

async fn process_statuses(
    rpc: &RpcClient,
    pending: &mut Vec<PendingConfirmation>,
    signatures: &[Signature],
    statuses: &[Option<solana_transaction_status::TransactionStatus>],
    risk_tracker: &SharedRiskTracker,
    risk_config: &RiskConfig,
    metrics: &SharedMetrics,
) {
    let mut to_remove: Vec<usize> = Vec::new();

    for (i, status_opt) in statuses.iter().enumerate() {
        let status = match status_opt {
            Some(s) => s,
            None => continue, // Not yet processed by the network, keep waiting
        };

        // status.status is Result<(), TransactionError>
        let succeeded = status.status.is_ok();
        let chain_err = if succeeded {
            String::new()
        } else {
            format!("{:?}", status.status)
        };
        let sig = &signatures[i];

        // R2-M02: track confirmation latency
        let latency_ms = pending[i].submitted_at.elapsed().as_millis() as u64;
        let submitted_slot = pending[i].submitted_slot;
        let current_slot = status.slot;
        let slot_delta = current_slot.saturating_sub(submitted_slot);
        if latency_ms > 30_000 {
            warn!(
                "[CONFIRM LATE] sig={} latency_ms={} submitted_slot={} confirmed_slot={} delta_slots={}",
                &pending[i].signature[..16.min(pending[i].signature.len())],
                latency_ms,
                submitted_slot,
                current_slot,
                slot_delta,
            );
            metrics.confirmations_late.inc();
        }

        // Extract actual SOL delta from on-chain transaction
        let actual_pnl = extract_actual_pnl(rpc, sig, &pending[i].wallet_pubkey).await;

        let p = &pending[i];
        match actual_pnl {
            Some(pnl) => {
                let mut rt = risk_tracker.lock().await;
                rt.record_confirmation(pnl, succeeded, risk_config);
                crate::persistence::trade_update_confirmed(&p.signature, pnl, succeeded);
                if succeeded {
                    metrics.trades_succeeded.inc();
                    metrics.profit_sol.add(pnl);
                }
                info!(
                    "[CONFIRM] sig={} route={} est: invest={:.6} meme={} sol_out={} | actual_pnl={:.6}",
                    &p.signature[..16.min(p.signature.len())],
                    p.route,
                    p.invest_sol,
                    p.est_meme,
                    p.est_sol_out,
                    pnl,
                );
            }
            None => {
                // Cannot extract on-chain balances (RPC error), fall back to estimate
                let pnl = if succeeded {
                    p.estimated_net_profit_sol
                } else {
                    // Failed: wasted priority fee + base fee
                    -(p.priority_fee_sol + 0.000_005)
                };
                let mut rt = risk_tracker.lock().await;
                rt.record_confirmation(pnl, succeeded, risk_config);
                crate::persistence::trade_update_confirmed(&p.signature, pnl, succeeded);
                if succeeded {
                    metrics.trades_succeeded.inc();
                    metrics.profit_sol.add(pnl);
                }
                warn!(
                    "[CONFIRM] sig={} route={} est: invest={:.6} meme={} sol_out={} | FAILED pnl_fallback={:.6} chain_err={}",
                    &p.signature[..16.min(p.signature.len())],
                    p.route,
                    p.invest_sol,
                    p.est_meme,
                    p.est_sol_out,
                    pnl,
                    chain_err,
                );
            }
        }

        to_remove.push(i);
    }

    // Remove processed entries from back to front
    for i in to_remove.into_iter().rev() {
        pending.swap_remove(i);
    }
}

/// Extract actual SOL PnL from a confirmed transaction (pre/post balance difference)
async fn extract_actual_pnl(
    rpc: &RpcClient,
    signature: &Signature,
    _wallet: &Pubkey,
) -> Option<f64> {
    use solana_client::rpc_config::RpcTransactionConfig;
    use solana_transaction_status::UiTransactionEncoding;

    let config = RpcTransactionConfig {
        encoding: Some(UiTransactionEncoding::Json),
        commitment: None,
        max_supported_transaction_version: Some(0),
    };

    // Retry up to 3 times (2s, 4s, 8s delays) — Shyft RPC needs time to index
    let mut tx = None;
    for attempt in 0u32..3u32 {
        match rpc.get_transaction_with_config(signature, config).await {
            Ok(t) => { tx = Some(t); break; }
            Err(_e) if attempt < 2 => {
                tokio::time::sleep(std::time::Duration::from_secs(2u64 << attempt)).await;
            }
            Err(e) => {
                log::debug!(
                    "[CONFIRM] RPC get_transaction failed sig={} after 3 attempts: {e}",
                    &signature.to_string()[..16.min(signature.to_string().len())],
                );
            }
        }
    }
    let tx = tx?;

    let meta = tx.transaction.meta.as_ref()?;

    let pre = meta.pre_balances.first()?;
    let post = meta.post_balances.first()?;

    // Account index 0 is the fee payer; its SOL balance change is the actual PnL
    let delta = (*post as i128) - (*pre as i128);
    Some(delta as f64 / 1_000_000_000.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_extract_pnl_calculation() {
        // Pure math verification: no RPC dependency
        let pre: u64 = 10_000_000_000; // 10 SOL
        let post: u64 = 10_050_000_000; // 10.05 SOL
        let delta = (post as i128) - (pre as i128);
        let pnl = delta as f64 / 1_000_000_000.0;
        assert!((pnl - 0.05).abs() < 1e-9);

        // Loss scenario: failed tx only pays the fee
        let post_loss: u64 = 9_999_995_000;
        let delta_loss = (post_loss as i128) - (pre as i128);
        let pnl_loss = delta_loss as f64 / 1_000_000_000.0;
        assert!((pnl_loss - (-0.000_005)).abs() < 1e-9);
    }
}
