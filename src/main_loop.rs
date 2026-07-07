//! Main event loop — receives swap events from listeners, scans for arbitrage,
//! and submits atomic transactions.

use log::{debug, info, warn};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::arbitrage::{ArbitrageScanner, Venue};
use crate::config::AppConfig;
use crate::executor::{PendingConfirmation, RpcPool};
use crate::grpc_stream;
use crate::listener::SwapEvent;
use crate::metrics::SharedMetrics;
use crate::risk::SharedRiskTracker;
use crate::whitelist::Whitelist;

pub(crate) struct EventLoopContext {
    pub rpc_pool: Arc<RpcPool>,
    pub whitelist: Arc<tokio::sync::RwLock<Whitelist>>,
    pub scanner: Arc<ArbitrageScanner>,
    pub config: AppConfig,
    pub risk_tracker: SharedRiskTracker,
    pub metrics: SharedMetrics,
    pub confirm_tx: mpsc::UnboundedSender<PendingConfirmation>,
    pub wallet: Option<Arc<solana_sdk::signature::Keypair>>,
    pub event_rx: mpsc::Receiver<SwapEvent>,
    pub cu_cost_sol: f64,
    pub priority_fee_sol: f64,
}

pub(crate) async fn run_event_loop(ctx: EventLoopContext) {
    let EventLoopContext {
        rpc_pool,
        whitelist,
        scanner,
        config,
        risk_tracker,
        metrics,
        confirm_tx,
        wallet,
        mut event_rx,
        cu_cost_sol,
        priority_fee_sol,
    } = ctx;
    let risk_config = Arc::new(config.risk.clone());
    let mut event_count: u64 = 0;

    // ---- Background active scanning (gRPC cache, 2s interval, requires config enable) ----
    if config.scanner.active_scan_enabled {
        let active_whitelist = whitelist.clone();
        let active_scanner = scanner.clone();
        let active_rpc_pool = rpc_pool.clone();
        let active_metrics = metrics.clone();
        let active_wallet = wallet.clone();
        let active_config = config.clone();
        tokio::spawn(async move {
            let mut scan_interval = tokio::time::interval(std::time::Duration::from_secs(2));
            let mut stats_interval = tokio::time::interval(std::time::Duration::from_secs(900));
            let mut scans_total: u64 = 0;
            let mut opps_found: u64 = 0;
            let mut opp_mints: std::collections::HashSet<String> = std::collections::HashSet::new();
            let t_start = std::time::Instant::now();
            loop {
                tokio::select! {
                    _ = scan_interval.tick() => {
                        if crate::grpc_stream::global_cache().latest_slot() == 0 { continue; }
                        let tokens: Vec<String> = {
                            let wl = active_whitelist.read().await;
                            wl.profitable.iter().chain(wl.verified.iter()).cloned().collect()
                        };
                        for mint in &tokens {
                            scans_total += 1;
                            let rpc = active_rpc_pool.current();
                            let t0 = std::time::Instant::now();
                            let opps = active_scanner.scan_by_mint(&rpc, mint).await;
                            let elapsed = t0.elapsed();
                            for opp in &opps {
                                if opp.net_profit_sol > 0.0 {
                                    opps_found += 1;
                                    opp_mints.insert(mint.clone());
                                    let sim_tag = if let Some(ref w) = active_wallet {
                                        match crate::executor::build_atomic_arbitrage_tx(opp, w, &active_config, &rpc).await {
                                            Ok((tx_bytes, _, _)) => {
                                                match crate::simulator::simulate_serialized_tx(&rpc, &tx_bytes).await {
                                                    Ok(()) => "OK".to_string(),
                                                    Err(e) => format!("FAIL:{}", e.to_string().chars().take(80).collect::<String>()),
                                                }
                                            }
                                            Err(e) => format!("BUILD:{}", e.to_string().chars().take(80).collect::<String>()),
                                        }
                                    } else { "NO_WALLET".to_string() };
                                    info!(
                                        "[ACTIVE OPP] mint={} buy={} sell={} diff={}bps net={:.6} invest={:.3} sim={} scan={:.0}ms",
                                        &mint[..12.min(mint.len())], opp.buy_venue.name(), opp.sell_venue.name(),
                                        opp.price_diff_bps, opp.net_profit_sol, opp.investment_sol, sim_tag, elapsed.as_millis(),
                                    );
                                }
                            }
                        }
                    }
                    _ = stats_interval.tick() => {
                        let uptime = t_start.elapsed().as_secs() / 60;
                        let wl_size = active_whitelist.read().await.len();
                        info!("[STATS] uptime={}min wl={} scans={} opps={} opp_mints={}",
                            uptime, wl_size, scans_total, opps_found, opp_mints.len());
                    }
                }
            }
        });
    }

    // ---- Main event loop ----
    loop {
        let swap: crate::listener::SwapEvent = tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutting down...");
                whitelist.read().await.save();
                let rt = risk_tracker.lock().await;
                info!(
                    "[RISK SUMMARY] daily_pnl={:.6} SOL total_pnl={:.6} SOL attempted={} pending={} succeeded={} failed_onchain={} wasted_fee={:.6} SOL breaker={}",
                    rt.daily_pnl_sol, rt.total_net_profit_sol,
                    rt.trades_attempted, rt.trades_pending, rt.trades_succeeded, rt.trades_failed_onchain,
                    rt.cumulative_failed_fee_sol, rt.circuit_breaker,
                );
                break;
            }
            event = event_rx.recv() => {
                match event {
                    Some(s) => s,
                    None => {
                        info!("[MAIN] WebSocket event channel closed");
                        break;
                    }
                }
            }
        };

        // Shared processing for both sources:
        event_count += 1;
        let t0 = std::time::Instant::now();
        let mint = match swap.mint {
            Some(ref m) => m.clone(),
            None => continue,
        };

                        // Check blacklist (db-managed, e.g. TransferFee incompatible tokens)
                        if whitelist.read().await.is_blacklisted(&mint) {
                            continue;
                        }

                        // H-03: get fresh client snapshot from pool (may have rotated)
                        let rpc = rpc_pool.current();

                        // Check both-side prices
                        let opps = scanner.scan_by_mint(&rpc, &mint).await;
                        let t1 = t0.elapsed();

                        for opp in &opps {
                            if opp.net_profit_sol < config.risk.min_profit_threshold_sol {
                                continue;
                            }

                            debug!(
                                "[OPPORTUNITY] mint={} buy={} sell={} diff={}bps invest={:.3} net={:.6}",
                                &mint[..12.min(mint.len())], opp.buy_venue.name(),
                                opp.sell_venue.name(), opp.price_diff_bps,
                                opp.investment_sol, opp.net_profit_sol,
                            );

                            // Risk control
                            {
                                let mut rt = risk_tracker.lock().await;
                                if !rt.check_limits(opp, &risk_config) {
                                    warn!("[RISK REJECTED] breaker={} daily_pnl={:.3}",
                                        rt.circuit_breaker, rt.daily_pnl_sol);
                                    continue;
                                }
                            }

                            metrics.opportunities_scanned.inc();

                            // M-07: reject opportunities whose price snapshot is too old.
                            // Slot from gRPC cache (zero-RTT), fall back to RPC if cache empty.
                            const MAX_SLOT_AGE: u64 = 20;

                            if config.bot.dry_run {
                                // Use gRPC slot for dry-run timing log, no RPC needed
                                let cur_slot = grpc_stream::global_cache().latest_slot();
                                info!("[DRY RUN] mint={} buy={} sell={} invest={:.3} net={:.6} timing=scan:{:.0}ms",
                                    &mint[..12.min(mint.len())], opp.buy_venue.name(),
                                    opp.sell_venue.name(), opp.investment_sol, opp.net_profit_sol,
                                    t1.as_millis());
                                if cur_slot > 0 && !crate::arbitrage::is_opportunity_fresh(opp.slot, cur_slot, MAX_SLOT_AGE) {
                                    debug!("[STALE OPP] mint={} age={}slots", &mint[..12.min(mint.len())], cur_slot.saturating_sub(opp.slot));
                                }
                                continue;
                            }

                            let w = match wallet.as_deref() {
                                Some(w) => w,
                                None => continue,
                            };

                            // M-07 slot + M-02 balance: parallel RPC (slot from gRPC cache if available)
                            let gprc_slot = grpc_stream::global_cache().latest_slot();
                            let wallet_pk = w.pubkey();
                            let (cur_slot, balance_result) = tokio::join!(
                                async {
                                    if gprc_slot > 0 {
                                        Ok(gprc_slot)
                                    } else {
                                        rpc.get_slot_with_commitment(
                                            solana_sdk::commitment_config::CommitmentConfig::processed(),
                                        )
                                        .await
                                    }
                                },
                                rpc.get_balance(&wallet_pk),
                            );
                            let cur_slot = match cur_slot {
                                Ok(s) => s,
                                Err(e) => {
                                    warn!("[SLOT FETCH FAIL] skipping mint={} error={}", &mint[..12.min(mint.len())], e);
                                    continue;
                                }
                            };
                            if !crate::arbitrage::is_opportunity_fresh(opp.slot, cur_slot, MAX_SLOT_AGE) {
                                debug!(
                                    "[STALE OPP] mint={} age={}slots — skipping",
                                    &mint[..12.min(mint.len())],
                                    cur_slot.saturating_sub(opp.slot),
                                );
                                continue;
                            }

                            // M-09: CU fee buffer — pad the cost estimate to protect against fee spikes
                            let buffer_pct = config.scanner.cu_fee_buffer_pct as f64 / 100.0;
                            // Native SOL only needs to cover tx fees + ATA rent.
                            // Investment SOL comes from pre-funded WSOL ATA.
                            let required_sol = cu_cost_sol * (1.0 + buffer_pct) + config.scanner.ata_rent_reserve_sol;
                            match balance_result {
                                Ok(lamports) => {
                                    let balance_sol = lamports as f64 / 1_000_000_000.0;
                                    if balance_sol < required_sol {
                                        warn!(
                                            "[BALANCE LOW] mint={} required={:.6} balance={:.6} invest={:.3} cu_cost={:.6} buffer={}%",
                                            &mint[..12.min(mint.len())],
                                            required_sol, balance_sol,
                                            opp.investment_sol, cu_cost_sol,
                                            config.scanner.cu_fee_buffer_pct,
                                        );
                                        continue;
                                    }
                                }
                                Err(e) => {
                                    warn!("[BALANCE FETCH FAIL] mint={} error={e} — skipping", &mint[..12.min(mint.len())]);
                                    continue;
                                }
                            }

                            // H-02: ensure WSOL ATA has enough SOL before building TX.
                            // If a wrap was submitted (fire-and-forget), skip this
                            // opportunity — WSOL won't arrive in time.
                            if let Some(ref wallet) = wallet {
                                let investment_lamports = crate::executor::atomic::helpers::sol_to_lamports(opp.investment_sol);
                                if ensure_wsol_balance(&rpc, wallet, &config, investment_lamports).await {
                                    continue; // wrap submitted, WSOL not yet available
                                }
                            }

                            let t_build = std::time::Instant::now();
                            let tx_result = crate::executor::build_atomic_arbitrage_tx(opp, w, &config, &rpc).await;
                            let t2 = t_build.elapsed();

                            match tx_result {
                                Ok((tx_bytes, last_valid_block_height, estimate)) => {
                                    let t_sim = if config.simulator.enabled {
                                        let t = std::time::Instant::now();
                                        let result = crate::executor::simulate_atomic_tx(&rpc, &tx_bytes).await;
                                        let elapsed = t.elapsed();
                                        if let Err(e) = result {
                                            warn!("[SIM FAILED]: sim={}ms {e}", elapsed.as_millis());
                                            let arb_prog_id = solana_sdk::pubkey::Pubkey::from_str(
                                                &config.execution_routing.onchain_program_id,
                                            ).unwrap_or_default();
                                            match crate::simulator::diagnostic_zero_slippage_sim(
                                                &rpc, &tx_bytes, &arb_prog_id,
                                            ).await {
                                                Ok(diag) => warn!("{}", diag),
                                                Err(diag_err) => warn!("[DIAGNOSTIC FAILED]: {diag_err}"),
                                            }
                                            continue;
                                        }
                                        Some(elapsed)
                                    } else {
                                        None
                                    };

                                    // Re-check slot age after build+simulate, gRPC or RPC fallback
                                    let pre_submit_slot = {
                                        let s = grpc_stream::global_cache().latest_slot();
                                        if s > 0 { s } else { rpc.get_slot_with_commitment(CommitmentConfig::processed()).await.unwrap_or(0) }
                                    };
                                    if pre_submit_slot == 0 {
                                        warn!("[PRE-SUBMIT SLOT FETCH FAIL] no slot source — skipping mint={}", &mint[..12.min(mint.len())]);
                                        continue;
                                    }
                                    if !crate::arbitrage::is_opportunity_fresh(opp.slot, pre_submit_slot, MAX_SLOT_AGE) {
                                        debug!(
                                            "[STALE PRE-SUBMIT] mint={} age={}slots — skipping",
                                            &mint[..12.min(mint.len())],
                                            pre_submit_slot.saturating_sub(opp.slot),
                                        );
                                        continue;
                                    }

                                    let t_submit = std::time::Instant::now();

                                    // Try Helius Sender first (SWQOS-only, tip=0.000005 SOL), fall back to RPC.
                                    // Try Jito bundle (direct Block Engine) first, fall back to RPC.
                                    let use_jito = config.solana.sender_enabled;
                                    let sig_result: anyhow::Result<String> = if use_jito {
                                        match crate::executor::submit_via_jito(&tx_bytes).await {
                                            Ok(bundle_id) => {
                                                info!("[JITO OK] bundle_id={bundle_id}");
                                                Ok(bundle_id)
                                            }
                                            Err(e) => {
                                                warn!("[JITO FAIL] falling back to RPC: {e}");
                                                crate::executor::submit_atomic_tx(&rpc, &tx_bytes, opp.slot, last_valid_block_height).await
                                            }
                                        }
                                    } else {
                                        crate::executor::submit_atomic_tx(&rpc, &tx_bytes, opp.slot, last_valid_block_height).await
                                    };

                                    match sig_result {
                                        Ok(sig) => {
                                            let t3 = t_submit.elapsed();
                                            info!("ARBITRAGE SUBMITTED sig={} buy={} sell={} est_net={:.6} timing=scan:{:.0}ms,build:{:.0}ms,sim:{:?}ms,submit:{:.0}ms,total:{:.0}ms",
                                                sig, opp.buy_venue.name(), opp.sell_venue.name(), opp.net_profit_sol,
                                                t1.as_millis(), t2.as_millis(),
                                                t_sim.map(|d| d.as_millis()).unwrap_or(0),
                                                t3.as_millis(),
                                                t0.elapsed().as_millis());
                                            whitelist.write().await.mark_profitable(&mint);
                                            {
                                                let wallet_pubkey = w.pubkey();
                                                let mut rt = risk_tracker.lock().await;
                                                rt.record_submitted();
                                                let route = match (opp.buy_venue, opp.sell_venue) {
                                                    (Venue::PumpSwapAmm, Venue::MeteoraDlmm) => "pump→dlmm",
                                                    (Venue::MeteoraDlmm, Venue::PumpSwapAmm) => "dlmm→pump",
                                                    _ => "other",
                                                };
                                                let (est_meme, est_sol_out) = estimate.as_ref()
                                                    .map(|e| (e.est_meme, e.est_sol_out))
                                                    .unwrap_or((0, 0));
                                                let _ = confirm_tx.send(PendingConfirmation {
                                                    signature: sig.clone(),
                                                    wallet_pubkey,
                                                    estimated_net_profit_sol: opp.net_profit_sol,
                                                    priority_fee_sol,
                                                    submitted_slot: pre_submit_slot,
                                                    submitted_at: std::time::Instant::now(),
                                                    invest_sol: opp.investment_sol,
                                                    est_meme,
                                                    est_sol_out,
                                                    route: route.to_string(),
                                                });
                                                // Write SQLite trade record (pending confirmation)
                                                crate::persistence::trade_insert_submitted(
                                                    &sig,
                                                    &opp.token_mint,
                                                    opp.buy_venue.name(),
                                                    opp.sell_venue.name(),
                                                    opp.investment_sol,
                                                    opp.net_profit_sol,
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            let t3 = t_submit.elapsed();
                                            let err_msg = format!("{e}");
                                            warn!("[SUBMIT FAILED] mint={} error={e} timing=scan:{:.0}ms,build:{:.0}ms,submit:{:.0}ms",
                                                &mint[..12.min(mint.len())],
                                                t1.as_millis(), t2.as_millis(), t3.as_millis());
                                            maybe_failover(&rpc_pool, &err_msg);
                                            {
                                                let mut rt = risk_tracker.lock().await;
                                                rt.trades_attempted += 1;
                                            }
                                            metrics.submissions_failed.inc();
                                        }
                                    }
                                }
                                Err(e) => {
                                    let err_msg = format!("{e}");
                                    warn!("[BUILD FAILED] mint={} error={e} timing=scan:{:.0}ms,build:{:.0}ms",
                                        &mint[..12.min(mint.len())], t1.as_millis(), t2.as_millis());
                                    maybe_failover(&rpc_pool, &err_msg);
                                },
                            }
                        }

                        if event_count.is_multiple_of(50) {
                            info!("[MAIN] events={} whitelist={}", event_count, whitelist.read().await.len());
                        }
    }
}

/// H-03: detect connection-related errors and trigger RPC failover.
fn maybe_failover(rpc_pool: &Arc<RpcPool>, error_msg: &str) {
    let is_connection_error = error_msg.contains("timed out")
        || error_msg.contains("connection")
        || error_msg.contains("Connection")
        || error_msg.contains("deadline")
        || error_msg.contains("error trying to connect")
        || error_msg.contains("Timeout");
    if is_connection_error {
        rpc_pool.rotate();
    }
}

/// H-02: prevent duplicate wrap TXs while one is already in-flight.
static WRAP_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// H-02: ensure WSOL ATA has enough balance for the investment.
/// Returns true if a wrap was submitted (caller should skip this opportunity).
async fn ensure_wsol_balance(
    rpc: &RpcClient,
    wallet: &Arc<solana_sdk::signature::Keypair>,
    config: &crate::config::AppConfig,
    investment_lamports: u64,
) -> bool {
    let wallet_pk = wallet.pubkey();
    let sol_mint = match Pubkey::from_str(crate::constants::NATIVE_SOL_MINT) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let sol_tp = match Pubkey::from_str(crate::constants::TOKEN_PROGRAM) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let wsol_ata = crate::simulator::ata_addr(&wallet_pk, &sol_mint, &sol_tp);

    // Read WSOL token balance
    let wsol_balance = match rpc.get_token_account_balance(&wsol_ata).await {
        Ok(b) => b.amount.parse::<f64>().unwrap_or(0.0),
        Err(_) => {
            // ATA doesn't exist yet — will be created by first TX's ATA create IX
            return false;
        }
    };
    let wsol_lamports = crate::executor::atomic::helpers::sol_to_lamports(wsol_balance);

    let buffer = crate::executor::atomic::helpers::sol_to_lamports(config.risk.max_single_investment_sol * 2.0);
    let required = investment_lamports.saturating_add(buffer);

    if wsol_lamports >= required {
        return false; // enough, no wrap needed
    }

    let deficit = required - wsol_lamports;
    let native_balance = rpc.get_balance(&wallet_pk).await.unwrap_or(0);
    let wrap_amount = deficit.min(native_balance.saturating_sub(100_000)); // keep 0.0001 SOL for fees

    if wrap_amount < 50_000_000 {
        if native_balance < investment_lamports {
            log::warn!(
                "[WSOL LOW] cannot replenish: native={:.6} WSOL={:.6} required={:.6}",
                native_balance as f64 / 1e9,
                wsol_lamports as f64 / 1e9,
                required as f64 / 1e9,
            );
        }
        return false;
    }

    // Prevent duplicate wrap submissions while one is in-flight
    if WRAP_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return false; // already submitting a wrap
    }

    log::info!(
        "[WSOL WRAP] wrapping {:.6} SOL → WSOL (native={:.6} WSOL={:.6} required={:.6})",
        wrap_amount as f64 / 1e9,
        native_balance as f64 / 1e9,
        wsol_lamports as f64 / 1e9,
        required as f64 / 1e9,
    );

    // Fire-and-forget: submit without waiting for confirmation.
    // Waiting for confirmation in the hot scan→build path would stall
    // the next opportunity (400-2000ms block time). The wrap TX will
    // likely land before the next scan cycle; if not, the next event
    // retries the balance check.
    let bh = match rpc.get_latest_blockhash().await {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mut tx = solana_sdk::transaction::Transaction::new_with_payer(
        &[
            solana_sdk::system_instruction::transfer(&wallet_pk, &wsol_ata, wrap_amount),
            solana_sdk::instruction::Instruction {
                program_id: sol_tp,
                accounts: vec![solana_sdk::instruction::AccountMeta::new(wsol_ata, false)],
                data: vec![17], // SyncNative
            },
        ],
        Some(&wallet_pk),
    );
    tx.sign(&[wallet.as_ref()], bh);
    match rpc.send_transaction(&tx).await {
        Ok(sig) => {
            log::info!("[WSOL WRAP] submitted: sig={} amount={:.6}", sig, wrap_amount as f64 / 1e9);
        }
        Err(e) => {
            log::warn!("[WSOL WRAP] send failed: {e}");
            WRAP_IN_FLIGHT.store(false, Ordering::Release);
            return false; // send failed, allow retry next event
        }
    }
    // Reset in-flight flag after a short delay (wrap TX should confirm quickly)
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        WRAP_IN_FLIGHT.store(false, Ordering::Release);
    });
    true // wrapped, skip this opportunity
}

pub(crate) async fn verify_dual_presence(rpc: &RpcClient, mint: &str) -> bool {
    use crate::pool_cache::PumpVenueKind;

    // Check all 4 venues in parallel — a mint is valid if any 2 different
    // venues have SOL-denominated pools. This enables CPMM↔Whirlpool and
    // other cross-venue routes beyond just PumpSwap↔DLMM.
    let sol = crate::constants::NATIVE_SOL_MINT;
    let (has_pumpswap, has_dlmm, has_cpmm, has_whirlpool) = tokio::join!(
        async {
            let bc = crate::pool_cache::fetch_bonding_curve(rpc, mint).await;
            matches!(bc, Some(s) if s.venue_kind == PumpVenueKind::PumpSwapPool)
        },
        async {
            let pools = crate::pool_cache::fetch_dlmm_by_mints(rpc, sol, mint, 0).await;
            !pools.is_empty()
        },
        async {
            crate::pool_cache::fetch_cpmm_now(rpc, sol, mint).await.is_some()
        },
        async {
            crate::pool_cache::fetch_whirlpool_by_mints(rpc, sol, mint).await.is_some()
        },
    );

    let count = has_pumpswap as u8 + has_dlmm as u8 + has_cpmm as u8 + has_whirlpool as u8;
    count >= 2
}
