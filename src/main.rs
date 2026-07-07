//! MEVBot entry point — Event-Driven atomic arbitrage
//!
//! Architecture (aligned 2026-06-14):
//!   Whitelist loading → Dual WebSocket listening (PumpSwap + DLMM)
//!   → swap event trigger → check both-side prices → price diff found → atomic TX submission
//!
//! No dependency on getProgramAccounts full scan, Jito Block Engine, or periodic polling.

mod arbitrage;
mod config;
mod constants;
mod discovery;
mod executor;
mod grpc_stream;
mod listener;
mod main_loop;
mod metrics;
mod persistence;
mod pool_cache;
mod price;
mod risk;
mod simulator;
mod whitelist;

use log::{error, info, warn};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::load().expect("Failed to load configuration");

    if let Some(ref proxy) = config.solana.proxy_url {
        std::env::set_var("HTTPS_PROXY", proxy);
        std::env::set_var("HTTP_PROXY", proxy);
        info!("Proxy configured: {}", proxy);
    }
    // GPAv2 discovery reads RPC URL from env via dotenvy (.env)
    // If not set by .env, use config value as fallback
    if std::env::var("SOLANA_RPC_URL").is_err() {
        std::env::set_var("SOLANA_RPC_URL", &config.solana.rpc_url);
    }

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("mevbot=debug,info"));

    let log_dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
        .join(".local/share/mevbot");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::daily(log_dir, "mevbot.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout)
        .with_target(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_target(false)
        .json();

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();
    tracing_log::LogTracer::init().ok();

    info!("MEVBot starting (event-driven + atomic TX)...");
    price::init();
    persistence::init_db();
    pool_cache::load_lb_pair_cache();
    pool_cache::load_cpmm_pools();
    pool_cache::load_whirlpool_pools();

    // ---- Whitelist ----
    let whitelist = whitelist::Whitelist::load();
    info!("[MAIN] whitelist loaded: {} mints", whitelist.len());

    // ---- RPC Pool (H-03: multi-endpoint failover) ----
    let rpc_pool = Arc::new(crate::executor::RpcPool::new(
        &config.solana.rpc_url,
        &config.solana.fallback_rpc_urls,
        Duration::from_secs(config.solana.rpc_timeout_secs),
    ));
    let rpc = rpc_pool.current();
    let block_height = rpc.get_block_height().await?;
    info!("Solana connected, block: {}", block_height);

    if !config.simulator.wallet_pubkey.is_empty() {
        if let Ok(wallet) = Pubkey::from_str(&config.simulator.wallet_pubkey) {
            if let Ok(lamports) = rpc.get_balance(&wallet).await {
                info!("Wallet {}: {:.4} SOL", wallet, lamports as f64 / 1e9);
            }
        }
    }

    let wallet: Option<Arc<solana_sdk::signature::Keypair>> = match config::load_keypair(&config) {
        Ok(kp) => {
            info!("Wallet loaded: {}", kp.pubkey());
            Some(Arc::new(kp))
        }
        Err(e) => {
            if config.bot.dry_run {
                warn!("Wallet not loaded (dry-run sim disabled): {e}");
                None
            } else {
                error!("Failed to load wallet: {e}");
                return Err(e);
            }
        }
    };

    // Pre-fund WSOL ATA (skip in dry_run — never submitting TX)
    if !config.bot.dry_run {
        if let Some(ref w) = wallet {
        let sol_mint = Pubkey::from_str(constants::NATIVE_SOL_MINT)?;
        let sol_tp = Pubkey::from_str(constants::TOKEN_PROGRAM)?;
        let wsol_ata = simulator::ata_addr(&w.pubkey(), &sol_mint, &sol_tp);
        match rpc.get_account(&wsol_ata).await {
            Ok(_) => {
                let bal = rpc.get_balance(&w.pubkey()).await.unwrap_or(0);
                info!("WSOL ATA exists, wallet balance: {:.6} SOL", bal as f64 / 1e9);
            }
            Err(_) => {
                // Create WSOL ATA + fund it
                let fund_lamports = crate::executor::atomic::helpers::sol_to_lamports(config.risk.max_single_investment_sol * 3.0);
                let bh = rpc.get_latest_blockhash().await?;
                let mut tx = solana_sdk::transaction::Transaction::new_with_payer(
                    &[
                        simulator::create_ata_idempotent_ix_v2(
                            &w.pubkey(), &wsol_ata, &w.pubkey(), &sol_mint, &sol_tp,
                        ),
                        solana_sdk::system_instruction::transfer(
                            &w.pubkey(), &wsol_ata, fund_lamports,
                        ),
                        solana_sdk::instruction::Instruction {
                            program_id: sol_tp,
                            accounts: vec![
                                solana_sdk::instruction::AccountMeta::new(wsol_ata, false),
                            ],
                            data: vec![17], // SyncNative
                        },
                    ],
                    Some(&w.pubkey()),
                );
                tx.sign(&[w.as_ref()], bh);
                match rpc.send_and_confirm_transaction(&tx).await {
                    Ok(sig) => info!("WSOL ATA funded: {} ({} SOL)", sig, fund_lamports as f64 / 1e9),
                    Err(e) => warn!("WSOL fund failed: {e} — first TX will fail, retry later"),
                }
            }
        }
    }
    }

    // gRPC pool state cache (Yellowstone) — 4 program subscription
    // Self-hosted localhost endpoint, no auth token needed
    {
        if let Ok(slot) = rpc.get_slot().await {
            grpc_stream::STARTUP_SLOT.store(slot, std::sync::atomic::Ordering::Relaxed);
            info!("[GRPC] startup slot={slot}, replay from ~10min ago");
        }
        let programs = vec![
            Pubkey::from_str(constants::PUMPFUN_AMM_PROGRAM)?,
            Pubkey::from_str(constants::DLMM_PROGRAM)?,
            Pubkey::from_str(constants::CPMM_PROGRAM)?,
            Pubkey::from_str(constants::WHIRLPOOL_PROGRAM)?,
        ];
        grpc_stream::spawn_grpc_subscription(
            config.grpc.endpoint.clone(),
            config.grpc.x_token.clone(),
            programs,
        );
    }

    // Pre-warm TP cache: batch-read all known DLMM pool reserve accounts
    // so TokenProgram detection works immediately without waiting for gRPC.
    crate::executor::atomic::warmup_tp_cache(&rpc).await;

    // Hot path only scans PumpSwap + DLMM (covers 88% of target BOT routes)
    // CPMM / Whirlpool code retained, enable after stable profitability
    let venues = vec![
        arbitrage::Venue::PumpSwapAmm,
        arbitrage::Venue::MeteoraDlmm,
        arbitrage::Venue::RaydiumCpmm,
        arbitrage::Venue::OrcaWhirlpool,
    ];

    // Estimate TX cost (SOL): priority fee = CU_price × CU_limit, plus base signature fee
    let cu_cost_sol = executor::atomic::compute_cu_cost_sol(&config.scanner);
    let priority_fee_sol = cu_cost_sol - 0.000_005; // base tx fee portion

    let scanner = Arc::new(arbitrage::ArbitrageScanner::new(
        config.risk.min_profit_threshold_sol,
        config.scanner.min_price_diff_bps,
        config.risk.max_single_investment_sol,
        config.scanner.min_pool_liquidity_sol,
        config.risk.max_tip_sol,
        venues,
        cu_cost_sol,
        priority_fee_sol,
        config.scanner.profit_safety_factor,
        config.dex.pumpfun_fee_bps,
        config.scanner.skip_reverify,
        config.scanner.max_pool_share,
        config.scanner.max_absolute_sol_out,
    ));

    let risk_tracker: risk::SharedRiskTracker =
        Arc::new(tokio::sync::Mutex::new(risk::RiskTracker::new()));
    let risk_config = Arc::new(config.risk.clone());
    let metrics: metrics::SharedMetrics = Arc::new(metrics::Metrics::new());
    metrics.whitelist_size.set(whitelist.len() as f64);
    metrics::start_metrics_server(metrics.clone(), config.monitoring.metrics_port);

    // Background confirmation task (H-02/H-03): on-chain confirmation → extract actual SOL delta → correct PnL
    // H-03: pass pool so confirmation task also benefits from failover
    let confirm_tx = executor::spawn_confirmation_task(
        rpc_pool.clone(),
        risk_tracker.clone(),
        risk_config.clone(),
        metrics.clone(),
    );

    // ---- Dual WebSocket listening ----
    let ws_url = if config.solana.ws_url.is_empty() {
        config
            .solana
            .rpc_url
            .replace("https://", "wss://")
            .replace("http://", "ws://")
    } else {
        config.solana.ws_url.clone()
    };
    let rpc_url = config.solana.rpc_url.clone();

    let whitelist = Arc::new(tokio::sync::RwLock::new(whitelist));
    let (discovery_tx, mut discovery_rx) = mpsc::unbounded_channel::<String>();
    // Clone for backscan before discovery_tx is moved into listener
    discovery::spawn_backscan(rpc_url.clone(), discovery_tx.clone());

    // RabbitStream disabled — token expired

    let event_rx = listener::run_dual_listen(
        &ws_url,
        &rpc_url,
        whitelist.clone(),
        discovery_tx,
        metrics.clone(),
    )
    .await?;
    info!("[MAIN] dual WebSocket active (PumpSwap + DLMM)");

    // ---- Background discovery task: batch-verify new mints and add to whitelist ----
    let discovery_whitelist = whitelist.clone();
    let discovery_rpc_pool = rpc_pool.clone();
    let discovery_metrics = metrics.clone();
    tokio::spawn(async move {
        let mut buf: Vec<String> = Vec::with_capacity(64);
        loop {
            // Collect up to 64 candidates or wait 5 seconds
            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                match tokio::time::timeout(remaining, discovery_rx.recv()).await {
                    Ok(Some(mint)) => {
                        buf.push(mint);
                        if buf.len() >= 64 {
                            break;
                        }
                    }
                    Ok(None) => return, // channel closed
                    Err(_) => break,    // timeout
                }
            }
            if buf.is_empty() {
                continue;
            }

            // Dedup then verify one by one
            buf.sort();
            buf.dedup_by(|a, b| a == b);
            for mint in buf.drain(..) {
                if discovery_whitelist.read().await.contains(&mint) {
                    continue; // Already added via another path
                }
                let drpc = discovery_rpc_pool.current();
                if main_loop::verify_dual_presence(&drpc, &mint).await {
                    let mut wl = discovery_whitelist.write().await;
                    if wl.verify_and_add(mint.clone()) {
                        wl.save();
                        let total = wl.len();
                        discovery_metrics.discovered_mints.inc();
                        discovery_metrics.whitelist_size.set(total as f64);
                        info!(
                            "[DISCOVERY] new mint: {} (total: {})",
                            &mint[..12.min(mint.len())],
                            total,
                        );
                    }
                }
                // 200ms interval to avoid hammering the RPC
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            }
        }
    });

    // ---- Main event loop ----
    main_loop::run_event_loop(main_loop::EventLoopContext {
        rpc_pool: rpc_pool.clone(),
        whitelist: whitelist.clone(),
        scanner,
        config,
        risk_tracker: risk_tracker.clone(),
        metrics,
        confirm_tx,
        wallet,
        event_rx,
        cu_cost_sol,
        priority_fee_sol,
    })
    .await;

    whitelist.read().await.save();
    info!("MEVBot stopped");
    Ok(())
}
