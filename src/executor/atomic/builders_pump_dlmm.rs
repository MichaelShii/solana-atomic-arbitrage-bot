//! Full TX builders: DLMM, Whirlpool, PumpSwap, CPMM combinations.

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

use super::generic_route::*;
use super::onchain_router::get_alt;
use super::compute_effective_slippage;


#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_dlmm_to_whirlpool_tx(
    opp: &ArbitrageOpportunity, wallet_pubkey: &Pubkey, wallet: &Keypair,
    config: &AppConfig, rpc: &RpcClient, sol_mint: &Pubkey, meme_mint: &Pubkey,
    token_program: &Pubkey, sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey, _user_meme_ata: &Pubkey,
    investment_lamports: u64, blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;
    let dlmm = pool_cache::get_dlmm_reserves(constants::NATIVE_SOL_MINT, &opp.token_mint)
        .context("DLMM reserves not cached")?;
    let wp_reserves = pool_cache::get_whirlpool_reserves(&sol_mint.to_string(), &meme_mint.to_string())
        .context("Whirlpool reserves not cached")?;
    let (_, fresh_bins) = tokio::join!(
        async { None::<f64> },
        pool_cache::fetch_bins_fresh(rpc, &dlmm.lb_pair),
    );
    let fresh_bins = fresh_bins?;
    let sol_is_x = dlmm.token_x_mint == constants::NATIVE_SOL_MINT;
    let dlmm_fee = opp.dlmm_fee_bps as f64 / 10000.0;
    let dlmm_est = simulator::estimate_dlmm_swap_output_full(
        &fresh_bins, investment_lamports, sol_is_x, dlmm_fee,
    );
    let meme_out_est = if dlmm_est.out == 0 {
        anyhow::bail!("DLMM estimate returned 0")
    } else { dlmm_est.out };
    let buy_slip = super::compute_effective_slippage(
        investment_lamports,
        if sol_is_x { dlmm.reserve_x } else { dlmm.reserve_y },
        config.risk.slippage_tolerance_bps,
    );
    let min_meme_out = (meme_out_est as f64 * buy_slip) as u64;
    let wp_meme = if wp_reserves.token_x_mint == sol_mint.to_string()
        { wp_reserves.reserve_y } else { wp_reserves.reserve_x };
    let wp_sol = if wp_reserves.token_x_mint == sol_mint.to_string()
        { wp_reserves.reserve_x } else { wp_reserves.reserve_y };
    let sol_out_est = checked_cp_swap_output(wp_meme, wp_sol, min_meme_out, wp_reserves.fee_rate as u64)
        .ok_or_else(|| anyhow::anyhow!("Whirlpool sell overflow"))?;
    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);
    let fresh_net = sol_out_est as f64 / 1e9 - opp.investment_sol - cu_cost_sol;
    if fresh_net < config.risk.min_profit_threshold_sol {
        anyhow::bail!("R2-M01 reject dlmm->wp: fresh_net={:.6}", fresh_net);
    }

    let user_meme_ata = &simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let dlmm_data = dlmm_section_data(&dlmm, meme_token_program)?;
    let wp_data = whirlpool_section_data(&wp_reserves)?;

    let arb_ix = build_generic_route(
        wallet_pubkey, user_sol_ata, user_meme_ata, meme_mint, sol_mint,
        investment_lamports, min_meme_out, 1,
        constants::DEX_KIND_DLMM, constants::DEX_KIND_WHIRLPOOL,
        None, None, None, Some(&dlmm_data),
        None, Some(&wp_data), None, None,
        dlmm_data.bin_count, 0, sol_is_x,
        meme_token_program, sol_token_program, config,
    ).await?;

    let mut ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_price(config.scanner.compute_unit_price_micro_lamports),
        ComputeBudgetInstruction::set_compute_unit_limit(config.scanner.compute_unit_limit),
        simulator::create_ata_idempotent_ix_v2(wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program),
        arb_ix,
    ];
    let alt_addr = Pubkey::from_str(config.execution_routing.onchain_arb_alt.as_deref().context("onchain_arb_alt")?)?;
    let alt = get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash).context("v0::Message")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet]).context("VersionedTransaction")?;
    log::info!("[ONCHAIN TX] dlmm->whirlpool mint={} invest={:.6} est_meme={} est_sol={}",
        &opp.token_mint[..12.min(opp.token_mint.len())], opp.investment_sol, meme_out_est, sol_out_est);
    Ok((bincode::serialize(&tx)?, meme_out_est, sol_out_est))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_whirlpool_to_dlmm_tx(
    opp: &ArbitrageOpportunity, wallet_pubkey: &Pubkey, wallet: &Keypair,
    config: &AppConfig, rpc: &RpcClient, sol_mint: &Pubkey, meme_mint: &Pubkey,
    token_program: &Pubkey, sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey, _user_meme_ata: &Pubkey,
    investment_lamports: u64, blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;
    let wp_reserves = pool_cache::get_whirlpool_reserves(&sol_mint.to_string(), &meme_mint.to_string())
        .context("Whirlpool reserves not cached")?;
    let dlmm = pool_cache::get_dlmm_reserves(constants::NATIVE_SOL_MINT, &opp.token_mint)
        .context("DLMM reserves not cached")?;
    let wp_sol = if wp_reserves.token_x_mint == sol_mint.to_string()
        { wp_reserves.reserve_x } else { wp_reserves.reserve_y };
    let wp_meme = if wp_reserves.token_x_mint == sol_mint.to_string()
        { wp_reserves.reserve_y } else { wp_reserves.reserve_x };
    let max_inv = wp_sol / 3;
    let investment_lamports = investment_lamports.min(max_inv.max(1));

    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);
    let meme_out_est = checked_cp_swap_output(wp_sol, wp_meme, investment_lamports, wp_reserves.fee_rate as u64)
        .ok_or_else(|| anyhow::anyhow!("Whirlpool buy overflow"))?;
    let buy_slip = super::compute_effective_slippage(investment_lamports, wp_sol, config.risk.slippage_tolerance_bps);
    let min_meme_out = (meme_out_est as f64 * buy_slip) as u64;

    let (_, fresh_bins) = tokio::join!(async { None::<f64> }, pool_cache::fetch_bins_fresh(rpc, &dlmm.lb_pair));
    let fresh_bins = fresh_bins?;
    let sol_is_x = dlmm.token_x_mint == constants::NATIVE_SOL_MINT;
    let dlmm_fee = opp.dlmm_fee_bps as f64 / 10000.0;
    // Sell meme→SOL: if SOL=x, we swap y→x (sell meme=token_y)
    let dlmm_est = simulator::estimate_dlmm_swap_output_full(
        &fresh_bins, min_meme_out, !sol_is_x, dlmm_fee,
    );
    let sol_out_est = if dlmm_est.out == 0 { min_meme_out / 2 } else { dlmm_est.out };
    let fresh_net = sol_out_est as f64 / 1e9 - opp.investment_sol - cu_cost_sol;
    if fresh_net < config.risk.min_profit_threshold_sol {
        anyhow::bail!("R2-M01 reject wp->dlmm: fresh_net={:.6}", fresh_net);
    }

    let user_meme_ata = &simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let wp_data = whirlpool_section_data(&wp_reserves)?;
    let dlmm_data = dlmm_section_data(&dlmm, meme_token_program)?;

    let arb_ix = build_generic_route(
        wallet_pubkey, user_sol_ata, user_meme_ata, meme_mint, sol_mint,
        investment_lamports, min_meme_out, 1,
        constants::DEX_KIND_WHIRLPOOL, constants::DEX_KIND_DLMM,
        None, Some(&wp_data), None, None,
        None, None, None, Some(&dlmm_data),
        0, dlmm_data.bin_count, true,
        meme_token_program, sol_token_program, config,
    ).await?;

    let mut ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_price(config.scanner.compute_unit_price_micro_lamports),
        ComputeBudgetInstruction::set_compute_unit_limit(config.scanner.compute_unit_limit),
        simulator::create_ata_idempotent_ix_v2(wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program),
        arb_ix,
    ];
    let alt_addr = Pubkey::from_str(config.execution_routing.onchain_arb_alt.as_deref().context("onchain_arb_alt")?)?;
    let alt = get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash).context("v0::Message")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet]).context("VersionedTransaction")?;
    log::info!("[ONCHAIN TX] whirlpool->dlmm mint={} invest={:.6} est_meme={} est_sol={}",
        &opp.token_mint[..12.min(opp.token_mint.len())], opp.investment_sol, meme_out_est, sol_out_est);
    Ok((bincode::serialize(&tx)?, meme_out_est, sol_out_est))
}

// ── Remaining 4 venue pairs: pump↔whirlpool, cpmm↔dlmm ───────────────

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_pump_to_whirlpool_tx(
    opp: &ArbitrageOpportunity, wallet_pubkey: &Pubkey, wallet: &Keypair,
    config: &AppConfig, rpc: &RpcClient, sol_mint: &Pubkey, meme_mint: &Pubkey,
    token_program: &Pubkey, sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey, _user_meme_ata: &Pubkey,
    investment_lamports: u64, blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;
    let pool = pool_cache::resolve_pumpswap_pool_address(rpc, &meme_mint.to_string()).await
        .ok_or_else(|| anyhow::anyhow!("pool not found"))?;
    let (pool_meta, fresh_sol, fresh_tok) =
        super::helpers::fetch_pumpswap_meta_and_reserves(rpc, &pool).await?;
    let wp = pool_cache::get_whirlpool_reserves(&sol_mint.to_string(), &meme_mint.to_string())
        .context("Whirlpool reserves not cached")?;
    let wp_sol = if wp.token_x_mint == sol_mint.to_string() { wp.reserve_x } else { wp.reserve_y };
    let wp_meme = if wp.token_x_mint == sol_mint.to_string() { wp.reserve_y } else { wp.reserve_x };

    let investment_lamports = investment_lamports.min((fresh_sol / 3).max(1));
    let cu = super::compute_cu_cost_sol(&config.scanner);
    let meme_out = simulator::checked_pumpswap_buy_output(
        investment_lamports, fresh_sol, fresh_tok, config.dex.pumpswap_fee_bps,
    ).ok_or_else(|| anyhow::anyhow!("PumpSwap buy overflow"))?;
    let slip = super::compute_effective_slippage(investment_lamports, fresh_sol, config.risk.slippage_tolerance_bps);
    let min_meme = (meme_out as f64 * slip) as u64;
    let sol_out = checked_cp_swap_output(wp_meme, wp_sol, min_meme, wp.fee_rate as u64)
        .ok_or_else(|| anyhow::anyhow!("Whirlpool sell overflow"))?;
    let net = sol_out as f64 / 1e9 - opp.investment_sol - cu;
    if net < config.risk.min_profit_threshold_sol { anyhow::bail!("R2-M01 pump->wp net={:.6}", net); }

    let user_meme_ata = &simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let pd = pumpswap_section_data(&pool_meta, &pool)?;
    let wd = whirlpool_section_data(&wp)?;
    let arb_ix = build_generic_route(
        wallet_pubkey, user_sol_ata, user_meme_ata, meme_mint, sol_mint,
        investment_lamports, min_meme, 1,
        constants::DEX_KIND_PUMPSWAP, constants::DEX_KIND_WHIRLPOOL,
        None, None, Some(&pd), None, None, Some(&wd), None, None,
        pd.remaining_count, 0, true, meme_token_program, sol_token_program, config,
    ).await?;
    let mut ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_price(config.scanner.compute_unit_price_micro_lamports),
        ComputeBudgetInstruction::set_compute_unit_limit(config.scanner.compute_unit_limit),
        simulator::create_ata_idempotent_ix_v2(wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program),
        arb_ix,
    ];
    let alt = get_alt(rpc, Pubkey::from_str(config.execution_routing.onchain_arb_alt.as_deref().context("alt")?)?).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash).context("v0")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet]).context("tx")?;
    log::info!("[ONCHAIN TX] pump->wp mint={} invest={:.6} est_meme={} est_sol={}",
        &opp.token_mint[..12.min(opp.token_mint.len())], opp.investment_sol, meme_out, sol_out);
    Ok((bincode::serialize(&tx)?, meme_out, sol_out))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_whirlpool_to_pump_tx(
    opp: &ArbitrageOpportunity, wallet_pubkey: &Pubkey, wallet: &Keypair,
    config: &AppConfig, rpc: &RpcClient, sol_mint: &Pubkey, meme_mint: &Pubkey,
    token_program: &Pubkey, sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey, _user_meme_ata: &Pubkey,
    investment_lamports: u64, blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;
    let wp = pool_cache::get_whirlpool_reserves(&sol_mint.to_string(), &meme_mint.to_string())
        .context("Whirlpool reserves not cached")?;
    let pool = pool_cache::resolve_pumpswap_pool_address(rpc, &meme_mint.to_string()).await
        .ok_or_else(|| anyhow::anyhow!("pool not found"))?;
    let (pool_meta, fresh_sol, fresh_tok) =
        super::helpers::fetch_pumpswap_meta_and_reserves(rpc, &pool).await?;
    let wp_sol = if wp.token_x_mint == sol_mint.to_string() { wp.reserve_x } else { wp.reserve_y };
    let wp_meme = if wp.token_x_mint == sol_mint.to_string() { wp.reserve_y } else { wp.reserve_x };

    let investment_lamports = investment_lamports.min((wp_sol / 3).max(1));
    let cu = super::compute_cu_cost_sol(&config.scanner);
    let meme_out = checked_cp_swap_output(wp_sol, wp_meme, investment_lamports, wp.fee_rate as u64)
        .ok_or_else(|| anyhow::anyhow!("Whirlpool buy overflow"))?;
    let slip = super::compute_effective_slippage(investment_lamports, wp_sol, config.risk.slippage_tolerance_bps);
    let min_meme = (meme_out as f64 * slip) as u64;
    let sol_out = simulator::checked_pumpswap_sell_output(
        min_meme, fresh_tok, fresh_sol, config.dex.pumpswap_fee_bps,
    ).ok_or_else(|| anyhow::anyhow!("PumpSwap sell overflow"))?;
    let net = sol_out as f64 / 1e9 - opp.investment_sol - cu;
    if net < config.risk.min_profit_threshold_sol { anyhow::bail!("R2-M01 wp->pump net={:.6}", net); }

    let user_meme_ata = &simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let wd = whirlpool_section_data(&wp)?;
    let pd = pumpswap_section_data(&pool_meta, &pool)?;
    let arb_ix = build_generic_route(
        wallet_pubkey, user_sol_ata, user_meme_ata, meme_mint, sol_mint,
        investment_lamports, min_meme, 1,
        constants::DEX_KIND_WHIRLPOOL, constants::DEX_KIND_PUMPSWAP,
        None, Some(&wd), None, None, None, None, Some(&pd), None,
        0, pd.remaining_count, true, meme_token_program, sol_token_program, config,
    ).await?;
    let mut ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_price(config.scanner.compute_unit_price_micro_lamports),
        ComputeBudgetInstruction::set_compute_unit_limit(config.scanner.compute_unit_limit),
        simulator::create_ata_idempotent_ix_v2(wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program),
        arb_ix,
    ];
    let alt = get_alt(rpc, Pubkey::from_str(config.execution_routing.onchain_arb_alt.as_deref().context("alt")?)?).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash).context("v0")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet]).context("tx")?;
    log::info!("[ONCHAIN TX] wp->pump mint={} invest={:.6} est_meme={} est_sol={}",
        &opp.token_mint[..12.min(opp.token_mint.len())], opp.investment_sol, meme_out, sol_out);
    Ok((bincode::serialize(&tx)?, meme_out, sol_out))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_cpmm_to_dlmm_tx(
    opp: &ArbitrageOpportunity, wallet_pubkey: &Pubkey, wallet: &Keypair,
    config: &AppConfig, rpc: &RpcClient, sol_mint: &Pubkey, meme_mint: &Pubkey,
    token_program: &Pubkey, sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey, _user_meme_ata: &Pubkey,
    investment_lamports: u64, blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;
    let cpmm = pool_cache::get_pool_state(&sol_mint.to_string(), &meme_mint.to_string())
        .or_else(|| pool_cache::get_pool_state_by_mint(&meme_mint.to_string()))
        .context("CPMM pool state not cached")?;
    let dlmm = pool_cache::get_dlmm_reserves(constants::NATIVE_SOL_MINT, &opp.token_mint)
        .context("DLMM reserves not cached")?;
    let cpmm_sol = if cpmm.token_0_mint == sol_mint.to_string() { cpmm.token_0_vault_raw } else { cpmm.token_1_vault_raw };
    let cpmm_meme = if cpmm.token_0_mint == sol_mint.to_string() { cpmm.token_1_vault_raw } else { cpmm.token_0_vault_raw };

    let cu = super::compute_cu_cost_sol(&config.scanner);
    let meme_out = checked_cp_swap_output(cpmm_sol, cpmm_meme, investment_lamports, CPMM_DEFAULT_TRADE_FEE_RATE)
        .ok_or_else(|| anyhow::anyhow!("CPMM buy overflow"))?;
    let slip = super::compute_effective_slippage(investment_lamports, cpmm_sol, config.risk.slippage_tolerance_bps);
    let min_meme = (meme_out as f64 * slip) as u64;

    let (_, fresh_bins) = tokio::join!(async { None::<f64> }, pool_cache::fetch_bins_fresh(rpc, &dlmm.lb_pair));
    let fresh_bins = fresh_bins?;
    let sol_is_x = dlmm.token_x_mint == constants::NATIVE_SOL_MINT;
    let dlmm_fee = opp.dlmm_fee_bps as f64 / 10000.0;
    let dlmm_est = simulator::estimate_dlmm_swap_output_full(&fresh_bins, min_meme, !sol_is_x, dlmm_fee);
    let sol_out = if dlmm_est.out == 0 { min_meme / 2 } else { dlmm_est.out };
    let net = sol_out as f64 / 1e9 - opp.investment_sol - cu;
    if net < config.risk.min_profit_threshold_sol { anyhow::bail!("R2-M01 cpmm->dlmm net={:.6}", net); }

    let user_meme_ata = &simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let cd = cpmm_section_data(&cpmm)?;
    let dd = dlmm_section_data(&dlmm, meme_token_program)?;
    let arb_ix = build_generic_route(
        wallet_pubkey, user_sol_ata, user_meme_ata, meme_mint, sol_mint,
        investment_lamports, min_meme, 1,
        constants::DEX_KIND_CPMM, constants::DEX_KIND_DLMM,
        Some(&cd), None, None, None, None, None, None, Some(&dd),
        0, dd.bin_count, true, meme_token_program, sol_token_program, config,
    ).await?;
    let mut ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_price(config.scanner.compute_unit_price_micro_lamports),
        ComputeBudgetInstruction::set_compute_unit_limit(config.scanner.compute_unit_limit),
        simulator::create_ata_idempotent_ix_v2(wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program),
        arb_ix,
    ];
    let alt = get_alt(rpc, Pubkey::from_str(config.execution_routing.onchain_arb_alt.as_deref().context("alt")?)?).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash).context("v0")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet]).context("tx")?;
    log::info!("[ONCHAIN TX] cpmm->dlmm mint={} invest={:.6} est_meme={} est_sol={}",
        &opp.token_mint[..12.min(opp.token_mint.len())], opp.investment_sol, meme_out, sol_out);
    Ok((bincode::serialize(&tx)?, meme_out, sol_out))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_dlmm_to_cpmm_tx(
    opp: &ArbitrageOpportunity, wallet_pubkey: &Pubkey, wallet: &Keypair,
    config: &AppConfig, rpc: &RpcClient, sol_mint: &Pubkey, meme_mint: &Pubkey,
    token_program: &Pubkey, sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey, _user_meme_ata: &Pubkey,
    investment_lamports: u64, blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;
    let dlmm = pool_cache::get_dlmm_reserves(constants::NATIVE_SOL_MINT, &opp.token_mint)
        .context("DLMM reserves not cached")?;
    let cpmm = pool_cache::get_pool_state(&sol_mint.to_string(), &meme_mint.to_string())
        .or_else(|| pool_cache::get_pool_state_by_mint(&meme_mint.to_string()))
        .context("CPMM pool state not cached")?;
    let cpmm_sol = if cpmm.token_0_mint == sol_mint.to_string() { cpmm.token_0_vault_raw } else { cpmm.token_1_vault_raw };
    let cpmm_meme = if cpmm.token_0_mint == sol_mint.to_string() { cpmm.token_1_vault_raw } else { cpmm.token_0_vault_raw };

    let (_, fresh_bins) = tokio::join!(async { None::<f64> }, pool_cache::fetch_bins_fresh(rpc, &dlmm.lb_pair));
    let fresh_bins = fresh_bins?;
    let sol_is_x = dlmm.token_x_mint == constants::NATIVE_SOL_MINT;
    let dlmm_fee = opp.dlmm_fee_bps as f64 / 10000.0;
    let dlmm_est = simulator::estimate_dlmm_swap_output_full(&fresh_bins, investment_lamports, sol_is_x, dlmm_fee);
    let meme_out = if dlmm_est.out == 0 { anyhow::bail!("DLMM estimate 0") } else { dlmm_est.out };
    let slip = super::compute_effective_slippage(investment_lamports,
        if sol_is_x { dlmm.reserve_x } else { dlmm.reserve_y }, config.risk.slippage_tolerance_bps);
    let min_meme = (meme_out as f64 * slip) as u64;

    let cu = super::compute_cu_cost_sol(&config.scanner);
    let sol_out = checked_cp_swap_output(cpmm_meme, cpmm_sol, min_meme, CPMM_DEFAULT_TRADE_FEE_RATE)
        .ok_or_else(|| anyhow::anyhow!("CPMM sell overflow"))?;
    let net = sol_out as f64 / 1e9 - opp.investment_sol - cu;
    if net < config.risk.min_profit_threshold_sol { anyhow::bail!("R2-M01 dlmm->cpmm net={:.6}", net); }

    let user_meme_ata = &simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let dd = dlmm_section_data(&dlmm, meme_token_program)?;
    let cd = cpmm_section_data(&cpmm)?;
    let arb_ix = build_generic_route(
        wallet_pubkey, user_sol_ata, user_meme_ata, meme_mint, sol_mint,
        investment_lamports, min_meme, 1,
        constants::DEX_KIND_DLMM, constants::DEX_KIND_CPMM,
        None, None, None, Some(&dd), Some(&cd), None, None, None,
        dd.bin_count, 0, sol_is_x, meme_token_program, sol_token_program, config,
    ).await?;
    let mut ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_price(config.scanner.compute_unit_price_micro_lamports),
        ComputeBudgetInstruction::set_compute_unit_limit(config.scanner.compute_unit_limit),
        simulator::create_ata_idempotent_ix_v2(wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program),
        arb_ix,
    ];
    let alt = get_alt(rpc, Pubkey::from_str(config.execution_routing.onchain_arb_alt.as_deref().context("alt")?)?).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash).context("v0")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet]).context("tx")?;
    log::info!("[ONCHAIN TX] dlmm->cpmm mint={} invest={:.6} est_meme={} est_sol={}",
        &opp.token_mint[..12.min(opp.token_mint.len())], opp.investment_sol, meme_out, sol_out);
    Ok((bincode::serialize(&tx)?, meme_out, sol_out))
}
