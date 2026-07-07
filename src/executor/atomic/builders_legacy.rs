//! Legacy on-chain TX builders: pump↔dlmm.

use anyhow::Context;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{v0, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::VersionedTransaction;
use std::str::FromStr;

use solana_client::nonblocking::rpc_client::RpcClient;

use crate::arbitrage::ArbitrageOpportunity;
use crate::config::AppConfig;
use crate::constants;
use crate::pool_cache;
use crate::simulator;

use super::compute_effective_slippage;
use super::helpers::{fetch_pumpswap_meta_and_reserves, pick_pumpswap_protocol_fee_recipient};
use super::onchain_router::{get_alt, build_route_pump_to_dlmm, build_route_dlmm_to_pump};

pub(crate) async fn build_onchain_pump_to_dlmm_tx(
    opp: &ArbitrageOpportunity,
    wallet_pubkey: &Pubkey,
    wallet: &Keypair,
    config: &AppConfig,
    rpc: &RpcClient,
    sol_mint: &Pubkey,
    meme_mint: &Pubkey,
    token_program: &Pubkey,
    sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey,
    _user_meme_ata: &Pubkey, // recomputed inside after TP override
    investment_lamports: u64,
    blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> { // (tx_bytes, est_meme, est_sol_out)
    let pool = pool_cache::resolve_pumpswap_pool_address(rpc, &meme_mint.to_string())
        .await
        .ok_or_else(|| anyhow::anyhow!("pool address not found for mint={}", meme_mint))?;

    let dlmm = pool_cache::get_dlmm_reserves(constants::NATIVE_SOL_MINT, &opp.token_mint)
        .context("DLMM reserves not cached")?;

    let meme_token_program = token_program;

    // R2-M01: pool meta + fresh reserves + fresh DLMM bins in parallel
    let (pumpswap_result, fresh_bins_result) = tokio::join!(
        fetch_pumpswap_meta_and_reserves(rpc, &pool),
        pool_cache::fetch_bins_fresh(rpc, &dlmm.lb_pair),
    );
    let (pool_meta, fresh_sol_res, fresh_tok_res) = pumpswap_result?;
    let fresh_bins = fresh_bins_result?;

    // Cap investment at 30% of pool SOL reserves to prevent PumpSwap
    // arithmetic overflow when the pool shrinks between scan and execution.
    let max_investment = fresh_sol_res / 3;
    let investment_lamports = if fresh_sol_res > 0 && investment_lamports > max_investment {
        log::warn!(
            "capped investment {} -> {} lamports ({}% pool) mint={}",
            investment_lamports,
            max_investment,
            (max_investment as f64 / fresh_sol_res as f64 * 100.0) as u32,
            &opp.token_mint[..12.min(opp.token_mint.len())],
        );
        max_investment
    } else {
        investment_lamports
    };

    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);

    // Pricing: PumpSwap buy with fresh reserves
    let meme_out_est = simulator::checked_pumpswap_buy_output(
        investment_lamports,
        fresh_sol_res,
        fresh_tok_res,
        config.dex.pumpswap_fee_bps,
    )
    .ok_or_else(|| {
        anyhow::anyhow!(
            "PumpSwap buy overflow mint={} sol_res={} tok_res={} inv={}",
            &opp.token_mint[..12.min(opp.token_mint.len())],
            fresh_sol_res,
            fresh_tok_res,
            investment_lamports,
        )
    })?;
    let buy_slippage = compute_effective_slippage(
        investment_lamports,
        fresh_sol_res,
        config.risk.slippage_tolerance_bps,
    );
    let min_meme_out = (meme_out_est as f64 * buy_slippage) as u64;

    // DLMM sell pricing
    let sell_amount = min_meme_out; // conservative second leg
    let (meme_reserve_for_sell, _sol_reserve_for_sell) =
        if dlmm.token_x_mint == constants::NATIVE_SOL_MINT {
            (dlmm.reserve_y, dlmm.reserve_x)
        } else {
            (dlmm.reserve_x, dlmm.reserve_y)
        };
    let _sell_slippage = compute_effective_slippage(
        sell_amount,
        meme_reserve_for_sell,
        config.risk.slippage_tolerance_bps,
    );
    let is_x_to_y = dlmm.token_x_mint == constants::NATIVE_SOL_MINT;
    let dlmm_fee_rate = opp.dlmm_fee_bps as f64 / 10000.0;
    let dlmm_est =
        simulator::estimate_dlmm_swap_output_full(&fresh_bins, sell_amount, !is_x_to_y, dlmm_fee_rate);
    let sol_out_est = if dlmm_est.out == 0 {
        anyhow::bail!(
            "DLMM bin estimate returned 0 — bins may be empty or corrupted mint={}",
            &opp.token_mint[..12.min(opp.token_mint.len())]
        );
    } else {
        dlmm_est.out
    };
    let min_profit_lamports = 1; // accept any positive net profit (competitor strategy)

    // Recompute user meme ATA with the correct token program.
    // The PDA depends on the token program; overriding TP changes the ATA address.
    let user_meme_ata_actual = simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let user_meme_ata = &user_meme_ata_actual;

    // R2-M01 freshness checks (pump→dlmm)
    let fresh_gross = sol_out_est as f64 / 1e9 - opp.investment_sol;
    let fresh_net = fresh_gross - cu_cost_sol;
    if fresh_net < config.risk.min_profit_threshold_sol {
        anyhow::bail!(
            "R2-M01 reject: fresh_net={:.6} < min_profit={:.6} mint={}",
            fresh_net,
            config.risk.min_profit_threshold_sol,
            &opp.token_mint[..12.min(opp.token_mint.len())],
        );
    }

    // Build onchain IX
    let (arb_ix, _) = build_route_pump_to_dlmm(
        wallet_pubkey,
        user_sol_ata,
        user_meme_ata,
        meme_mint,
        investment_lamports,
        min_meme_out,
        min_profit_lamports,
        &dlmm,
        &pool_meta,
        &pool,
        meme_token_program,
        config,
        0,
    )
    .await?;

    // Assemble TX: CU + ATA + WSOL wrap + arb IX + WSOL close
    let mut ixs: Vec<Instruction> = Vec::new();
    ixs.push(ComputeBudgetInstruction::set_compute_unit_price(
        config.scanner.compute_unit_price_micro_lamports,
    ));
    ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(
        config.scanner.compute_unit_limit,
    ));
    ixs.push(simulator::create_ata_idempotent_ix_v2(
        wallet_pubkey,
        user_meme_ata,
        wallet_pubkey,
        meme_mint,
        meme_token_program,
    ));
    ixs.push(arb_ix);

    // ── ALT-driven v0 message (reduces 44-account TX under 1232 bytes) ─
    let alt_addr = Pubkey::from_str(
        config
            .execution_routing
            .onchain_arb_alt
            .as_deref()
            .context("onchain_arb_alt not configured — cannot build v0 TX")?,
    )?;
    let alt = get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash)
        .context("v0::Message::try_compile")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet])
        .context("VersionedTransaction::try_new")?;

    // Log consumed bins for pre/post comparison
    let est_bins: Vec<String> = dlmm_est.consumed_bins
        .iter()
        .map(|(id, ri, ro)| format!("bin={} in={} out={}", id, ri, ro))
        .collect();
    log::info!(
        "[ONCHAIN TX] pumpswap→dlmm mint={} invest={:.6} SOL est_meme={} est_sol_out={} bins_used={}/{} consumed=[{}]",
        &opp.token_mint[..12.min(opp.token_mint.len())],
        opp.investment_sol,
        meme_out_est,
        sol_out_est,
        dlmm_est.bins_consumed,
        dlmm_est.bins_total,
        est_bins.join(", "),
    );

    let tx_bytes = bincode::serialize(&tx).context("serialize onchain tx")?;
    Ok((tx_bytes, meme_out_est, sol_out_est))
}

/// Build full on-chain TX: DLMM buy → PumpSwap AMM sell
#[allow(clippy::too_many_arguments, clippy::vec_init_then_push)]
pub(crate) async fn build_onchain_dlmm_to_pump_tx(
    opp: &ArbitrageOpportunity,
    wallet_pubkey: &Pubkey,
    wallet: &Keypair,
    config: &AppConfig,
    rpc: &RpcClient,
    sol_mint: &Pubkey,
    meme_mint: &Pubkey,
    token_program: &Pubkey,
    sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey,
    _user_meme_ata: &Pubkey, // recomputed inside after TP override
    investment_lamports: u64,
    blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> { // (tx_bytes, est_meme, est_sol_out)
    let pool = pool_cache::resolve_pumpswap_pool_address(rpc, &meme_mint.to_string())
        .await
        .ok_or_else(|| anyhow::anyhow!("pool address not found for mint={}", meme_mint))?;

    let dlmm = pool_cache::get_dlmm_reserves(constants::NATIVE_SOL_MINT, &opp.token_mint)
        .context("DLMM reserves not cached")?;

    let meme_token_program = token_program;

    // R2-M01: pool meta + fresh reserves + fresh DLMM bins in parallel
    let (pumpswap_result, fresh_bins_result) = tokio::join!(
        fetch_pumpswap_meta_and_reserves(rpc, &pool),
        pool_cache::fetch_bins_fresh(rpc, &dlmm.lb_pair),
    );
    let (pool_meta, fresh_sol_res, fresh_tok_res) = pumpswap_result?;
    let fresh_bins = fresh_bins_result?;

    // Cap investment at 30% of pool SOL reserves to prevent PumpSwap
    // arithmetic overflow when the pool shrinks between scan and execution.
    let max_investment = fresh_sol_res / 3;
    let investment_lamports = if fresh_sol_res > 0 && investment_lamports > max_investment {
        log::warn!(
            "capped investment {} -> {} lamports ({}% pool) mint={}",
            investment_lamports,
            max_investment,
            (max_investment as f64 / fresh_sol_res as f64 * 100.0) as u32,
            &opp.token_mint[..12.min(opp.token_mint.len())],
        );
        max_investment
    } else {
        investment_lamports
    };

    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);

    // Pricing: DLMM buy
    let sol_is_x = dlmm.token_x_mint == constants::NATIVE_SOL_MINT;
    let (sol_reserve, meme_reserve_for_buy) = if sol_is_x {
        (dlmm.reserve_x, dlmm.reserve_y)
    } else {
        (dlmm.reserve_y, dlmm.reserve_x)
    };
    let buy_slippage = compute_effective_slippage(
        investment_lamports,
        sol_reserve,
        config.risk.slippage_tolerance_bps,
    );
    let dlmm_fee_rate = opp.dlmm_fee_bps as f64 / 10000.0;
    let dlmm_est = simulator::estimate_dlmm_swap_output_full(
        &fresh_bins,
        investment_lamports,
        sol_is_x,
        dlmm_fee_rate,
    );
    let meme_out_est = if dlmm_est.out == 0 {
        anyhow::bail!(
            "DLMM bin estimate returned 0 — bins may be empty or corrupted mint={}",
            &opp.token_mint[..12.min(opp.token_mint.len())]
        );
    } else {
        dlmm_est.out
    };
    let min_meme_out = (meme_out_est as f64 * buy_slippage) as u64;
    // min_intermediate_meme is used as PumpSwap sell's min_amount_out in the Router.
    // Set to 1 (execute-then-check) since post-CPI invariant already catches losses.
    // The actual sell amount is determined on-chain from the DLMM buy output.
    let min_sol_out = 1u64;

    // PumpSwap sell pricing with fresh reserves
    let sell_amount = min_meme_out; // conservative second leg
    let _sell_slippage = compute_effective_slippage(
        sell_amount,
        fresh_tok_res,
        config.risk.slippage_tolerance_bps,
    );
    let sol_out_est = simulator::checked_pumpswap_sell_output(
        sell_amount,
        fresh_tok_res,
        fresh_sol_res,
        config.dex.pumpswap_fee_bps,
    )
    .ok_or_else(|| {
        anyhow::anyhow!(
            "PumpSwap sell overflow mint={} sol_res={} tok_res={} sell={}",
            &opp.token_mint[..12.min(opp.token_mint.len())],
            fresh_sol_res,
            fresh_tok_res,
            sell_amount,
        )
    })?;
    let min_profit_lamports = 1; // accept any positive net profit (competitor strategy)

    // Recompute user meme ATA with the correct token program.
    // The PDA depends on the token program; overriding TP changes the ATA address.
    let user_meme_ata_actual = simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let user_meme_ata = &user_meme_ata_actual;

    // R2-M01 freshness checks (dlmm→pump)
    let fresh_gross = sol_out_est as f64 / 1e9 - opp.investment_sol;
    let fresh_net = fresh_gross - cu_cost_sol;
    if fresh_net < config.risk.min_profit_threshold_sol {
        anyhow::bail!(
            "R2-M01 reject: fresh_net={:.6} < min_profit={:.6} mint={}",
            fresh_net,
            config.risk.min_profit_threshold_sol,
            &opp.token_mint[..12.min(opp.token_mint.len())],
        );
    }
    let safety_floor = opp.net_profit_sol * 0.5;
    if fresh_net < safety_floor {
        anyhow::bail!(
            "R2-M01 reject: fresh_net={:.6} < 50% original_net={:.6} mint={}",
            fresh_net,
            opp.net_profit_sol,
            &opp.token_mint[..12.min(opp.token_mint.len())],
        );
    }

    // Build onchain IX
    let (arb_ix, _) = build_route_dlmm_to_pump(
        wallet_pubkey,
        user_sol_ata,
        user_meme_ata,
        meme_mint,
        investment_lamports,
        min_sol_out,
        min_profit_lamports,
        &dlmm,
        &pool_meta,
        &pool,
        meme_token_program,
        config,
        0,
    )
    .await?;

    // Assemble TX: CU + ATA + WSOL wrap + arb IX + WSOL close
    let mut ixs: Vec<Instruction> = Vec::new();
    ixs.push(ComputeBudgetInstruction::set_compute_unit_price(
        config.scanner.compute_unit_price_micro_lamports,
    ));
    ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(
        config.scanner.compute_unit_limit,
    ));
    ixs.push(simulator::create_ata_idempotent_ix_v2(
        wallet_pubkey,
        user_meme_ata,
        wallet_pubkey,
        meme_mint,
        meme_token_program,
    ));
    ixs.push(arb_ix);

    // ── ALT-driven v0 message (reduces 42-account TX under 1232 bytes) ─
    let alt_addr = Pubkey::from_str(
        config
            .execution_routing
            .onchain_arb_alt
            .as_deref()
            .context("onchain_arb_alt not configured — cannot build v0 TX")?,
    )?;
    let alt = get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash)
        .context("v0::Message::try_compile")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet])
        .context("VersionedTransaction::try_new")?;

    let est_bins: Vec<String> = dlmm_est.consumed_bins
        .iter()
        .map(|(id, ri, ro)| format!("bin={} in={} out={}", id, ri, ro))
        .collect();
    log::info!(
        "[ONCHAIN TX] dlmm→pumpswap mint={} invest={:.6} SOL est_meme={} est_sol_out={} bins_used={}/{} consumed=[{}]",
        &opp.token_mint[..12.min(opp.token_mint.len())],
        opp.investment_sol,
        meme_out_est,
        sol_out_est,
        dlmm_est.bins_consumed,
        dlmm_est.bins_total,
        est_bins.join(", "),
    );

    let tx_bytes = bincode::serialize(&tx).context("serialize onchain tx")?;
    Ok((tx_bytes, meme_out_est, sol_out_est))
}

