//! Full TX builders: CPMM, Whirlpool, PumpSwap combinations.

use anyhow::Context;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::{v0, VersionedMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
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

// ── Full TX builders for CPMM / Whirlpool routes ────────────────────────

/// Build full on-chain TX: CPMM buy → Whirlpool sell
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_cpmm_to_whirlpool_tx(
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
    _user_meme_ata: &Pubkey,
    investment_lamports: u64,
    blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;

    // 1. Fetch CPMM pool data
    let cpmm_state = pool_cache::get_pool_state(&sol_mint.to_string(), &meme_mint.to_string())
        .or_else(|| pool_cache::get_pool_state_by_mint(&meme_mint.to_string()))
        .context("CPMM pool state not cached")?;

    // 2. Fetch Whirlpool pool data
    let wp_reserves = pool_cache::get_whirlpool_reserves(
        &sol_mint.to_string(),
        &meme_mint.to_string(),
    )
    .or_else(|| {
        // Try via fresh fetch
        None
    })
    .context("Whirlpool reserves not cached")?;

    // 3. Determine CPMM side (which vault is SOL, which is meme)
    let (cpmm_sol_vault_raw, cpmm_meme_vault_raw) =
        if cpmm_state.token_0_mint == sol_mint.to_string() {
            (cpmm_state.token_0_vault_raw, cpmm_state.token_1_vault_raw)
        } else {
            (cpmm_state.token_1_vault_raw, cpmm_state.token_0_vault_raw)
        };

    // 4. Determine Whirlpool side
    let (wp_sol_reserve, wp_meme_reserve) =
        if wp_reserves.token_x_mint == sol_mint.to_string() {
            (wp_reserves.reserve_x, wp_reserves.reserve_y)
        } else {
            (wp_reserves.reserve_y, wp_reserves.reserve_x)
        };

    // 5. Pricing: CPMM buy (SOL → meme) — Raydium subtractive fee model.
    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);
    let meme_out_est = checked_cp_swap_output(
        cpmm_sol_vault_raw,
        cpmm_meme_vault_raw,
        investment_lamports,
        CPMM_DEFAULT_TRADE_FEE_RATE,
    )
    .ok_or_else(|| anyhow::anyhow!("CPMM buy overflow"))?;
    let buy_slippage = super::compute_effective_slippage(
        investment_lamports,
        cpmm_sol_vault_raw,
        config.risk.slippage_tolerance_bps,
    );
    let min_meme_out = (meme_out_est as f64 * buy_slippage) as u64;

    // 7. Pricing: Whirlpool sell (meme → SOL) — subtractive fee model.
    // fee_rate field is hundredths of a bp (3000 = 0.30%), denominator 1_000_000.
    let sol_out_est = checked_cp_swap_output(
        wp_meme_reserve,
        wp_sol_reserve,
        min_meme_out,
        wp_reserves.fee_rate as u64,
    )
    .ok_or_else(|| anyhow::anyhow!("Whirlpool sell overflow"))?;
    let min_profit_lamports = 1u64;

    // 8. Freshness check
    let fresh_gross = sol_out_est as f64 / 1_000_000_000.0 - opp.investment_sol;
    let fresh_net = fresh_gross - cu_cost_sol;
    if fresh_net < config.risk.min_profit_threshold_sol {
        anyhow::bail!(
            "R2-M01 reject cpmm→wp: fresh_net={:.6} < min_profit={:.6}",
            fresh_net, config.risk.min_profit_threshold_sol,
        );
    }

    // 9. Recompute user meme ATA
    let user_meme_ata_actual = simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let user_meme_ata = &user_meme_ata_actual;

    // 10. Build section data
    let cpmm_buy = cpmm_section_data(&cpmm_state)?;
    let wp_sell = whirlpool_section_data(&wp_reserves)?;

    // 11. Build onchain IX
    let arb_ix = build_generic_route(
        wallet_pubkey,
        user_sol_ata,
        user_meme_ata,
        meme_mint,
        sol_mint,
        investment_lamports,
        min_meme_out,
        min_profit_lamports,
        constants::DEX_KIND_CPMM,
        constants::DEX_KIND_WHIRLPOOL,
        Some(&cpmm_buy), None, None, None,
        None, Some(&wp_sell), None, None,
        0, 0, true,
        meme_token_program, sol_token_program, config,
    )
    .await?;

    // 12. Assemble TX
    let mut ixs: Vec<Instruction> = Vec::new();
    ixs.push(ComputeBudgetInstruction::set_compute_unit_price(
        config.scanner.compute_unit_price_micro_lamports,
    ));
    ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(
        config.scanner.compute_unit_limit,
    ));
    ixs.push(simulator::create_ata_idempotent_ix_v2(
        wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program,
    ));
    ixs.push(arb_ix);

    let alt_addr = Pubkey::from_str(
        config.execution_routing.onchain_arb_alt.as_deref()
            .context("onchain_arb_alt not configured")?,
    )?;
    let alt = get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash)
        .context("v0::Message::try_compile")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet])
        .context("VersionedTransaction::try_new")?;

    log::info!(
        "[ONCHAIN TX] cpmm→whirlpool mint={} invest={:.6} SOL est_meme={} est_sol_out={}",
        &opp.token_mint[..12.min(opp.token_mint.len())],
        opp.investment_sol,
        meme_out_est,
        sol_out_est,
    );

    let tx_bytes = bincode::serialize(&tx).context("serialize onchain tx")?;
    Ok((tx_bytes, meme_out_est, sol_out_est))
}

/// Build full on-chain TX: Whirlpool buy → CPMM sell
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_whirlpool_to_cpmm_tx(
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
    _user_meme_ata: &Pubkey,
    investment_lamports: u64,
    blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;

    // 1. Fetch pool data
    let cpmm_state = pool_cache::get_pool_state(&sol_mint.to_string(), &meme_mint.to_string())
        .or_else(|| pool_cache::get_pool_state_by_mint(&meme_mint.to_string()))
        .context("CPMM pool state not cached")?;
    let wp_reserves = pool_cache::get_whirlpool_reserves(
        &sol_mint.to_string(),
        &meme_mint.to_string(),
    )
    .context("Whirlpool reserves not cached")?;

    // 2. Determine pool sides
    let (cpmm_sol_vault_raw, cpmm_meme_vault_raw) =
        if cpmm_state.token_0_mint == sol_mint.to_string() {
            (cpmm_state.token_0_vault_raw, cpmm_state.token_1_vault_raw)
        } else {
            (cpmm_state.token_1_vault_raw, cpmm_state.token_0_vault_raw)
        };
    let (wp_sol_reserve, wp_meme_reserve) =
        if wp_reserves.token_x_mint == sol_mint.to_string() {
            (wp_reserves.reserve_x, wp_reserves.reserve_y)
        } else {
            (wp_reserves.reserve_y, wp_reserves.reserve_x)
        };

    // Cap investment
    let max_investment = wp_sol_reserve / 3;
    let investment_lamports = if wp_sol_reserve > 0 && investment_lamports > max_investment {
        investment_lamports.min(max_investment)
    } else {
        investment_lamports
    };

    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);

    // 3. Pricing: Whirlpool buy (SOL → meme) — subtractive fee model.
    // fee_rate field is hundredths of a bp, denominator 1_000_000.
    let meme_out_est = checked_cp_swap_output(
        wp_sol_reserve,
        wp_meme_reserve,
        investment_lamports,
        wp_reserves.fee_rate as u64,
    )
    .ok_or_else(|| anyhow::anyhow!("Whirlpool buy overflow"))?;
    let buy_slippage = super::compute_effective_slippage(
        investment_lamports,
        wp_sol_reserve,
        config.risk.slippage_tolerance_bps,
    );
    let min_meme_out = (meme_out_est as f64 * buy_slippage) as u64;

    // 4. Pricing: CPMM sell (meme → SOL) — Raydium subtractive fee model.
    let sol_out_est = checked_cp_swap_output(
        cpmm_meme_vault_raw,
        cpmm_sol_vault_raw,
        min_meme_out,
        CPMM_DEFAULT_TRADE_FEE_RATE,
    )
    .ok_or_else(|| anyhow::anyhow!("CPMM sell overflow"))?;
    let min_profit_lamports = 1u64;

    // 5. Freshness check
    let fresh_gross = sol_out_est as f64 / 1_000_000_000.0 - opp.investment_sol;
    let fresh_net = fresh_gross - cu_cost_sol;
    if fresh_net < config.risk.min_profit_threshold_sol {
        anyhow::bail!(
            "R2-M01 reject wp→cpmm: fresh_net={:.6} < min_profit={:.6}",
            fresh_net, config.risk.min_profit_threshold_sol,
        );
    }

    // 6. Recompute user meme ATA
    let user_meme_ata_actual = simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let user_meme_ata = &user_meme_ata_actual;

    // 7. Build section data
    let wp_buy = whirlpool_section_data(&wp_reserves)?;
    let cpmm_sell = cpmm_section_data(&cpmm_state)?;

    // 8. Build onchain IX
    let arb_ix = build_generic_route(
        wallet_pubkey,
        user_sol_ata,
        user_meme_ata,
        meme_mint,
        sol_mint,
        investment_lamports,
        min_meme_out,
        min_profit_lamports,
        constants::DEX_KIND_WHIRLPOOL,
        constants::DEX_KIND_CPMM,
        None, Some(&wp_buy), None, None,
        Some(&cpmm_sell), None, None, None,
        0, 0, true,
        meme_token_program, sol_token_program, config,
    )
    .await?;

    // 9. Assemble TX
    let mut ixs: Vec<Instruction> = Vec::new();
    ixs.push(ComputeBudgetInstruction::set_compute_unit_price(
        config.scanner.compute_unit_price_micro_lamports,
    ));
    ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(
        config.scanner.compute_unit_limit,
    ));
    ixs.push(simulator::create_ata_idempotent_ix_v2(
        wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program,
    ));
    ixs.push(arb_ix);

    let alt_addr = Pubkey::from_str(
        config.execution_routing.onchain_arb_alt.as_deref()
            .context("onchain_arb_alt not configured")?,
    )?;
    let alt = get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash)
        .context("v0::Message::try_compile")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet])
        .context("VersionedTransaction::try_new")?;

    log::info!(
        "[ONCHAIN TX] whirlpool→cpmm mint={} invest={:.6} SOL est_meme={} est_sol_out={}",
        &opp.token_mint[..12.min(opp.token_mint.len())],
        opp.investment_sol,
        meme_out_est,
        sol_out_est,
    );

    let tx_bytes = bincode::serialize(&tx).context("serialize onchain tx")?;
    Ok((tx_bytes, meme_out_est, sol_out_est))
}

// ── Full TX builders for pump↔cpmm and dlmm↔whirlpool ─────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_pump_to_cpmm_tx(
    opp: &ArbitrageOpportunity, wallet_pubkey: &Pubkey, wallet: &Keypair,
    config: &AppConfig, rpc: &RpcClient, sol_mint: &Pubkey, meme_mint: &Pubkey,
    token_program: &Pubkey, sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey, _user_meme_ata: &Pubkey,
    investment_lamports: u64, blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;
    let pool = pool_cache::resolve_pumpswap_pool_address(rpc, &meme_mint.to_string()).await
        .ok_or_else(|| anyhow::anyhow!("pool address not found"))?;
    let (pool_meta, fresh_sol_res, fresh_tok_res) =
        super::helpers::fetch_pumpswap_meta_and_reserves(rpc, &pool).await?;
    let cpmm_state = pool_cache::get_pool_state(&sol_mint.to_string(), &meme_mint.to_string())
        .or_else(|| pool_cache::get_pool_state_by_mint(&meme_mint.to_string()))
        .context("CPMM pool state not cached")?;
    let cpmm_sol_vault = if cpmm_state.token_0_mint == sol_mint.to_string()
        { cpmm_state.token_0_vault_raw } else { cpmm_state.token_1_vault_raw };
    let cpmm_meme_vault = if cpmm_state.token_0_mint == sol_mint.to_string()
        { cpmm_state.token_1_vault_raw } else { cpmm_state.token_0_vault_raw };

    let max_inv = fresh_sol_res / 3;
    let investment_lamports = investment_lamports.min(max_inv.max(1));
    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);

    let meme_out_est = simulator::checked_pumpswap_buy_output(
        investment_lamports, fresh_sol_res, fresh_tok_res, config.dex.pumpswap_fee_bps,
    ).ok_or_else(|| anyhow::anyhow!("PumpSwap buy overflow"))?;
    let buy_slip = super::compute_effective_slippage(investment_lamports, fresh_sol_res, config.risk.slippage_tolerance_bps);
    let min_meme_out = (meme_out_est as f64 * buy_slip) as u64;

    let sol_out_est = checked_cp_swap_output(cpmm_meme_vault, cpmm_sol_vault, min_meme_out, CPMM_DEFAULT_TRADE_FEE_RATE)
        .ok_or_else(|| anyhow::anyhow!("CPMM sell overflow"))?;
    let fresh_net = sol_out_est as f64 / 1e9 - opp.investment_sol - cu_cost_sol;
    if fresh_net < config.risk.min_profit_threshold_sol {
        anyhow::bail!("R2-M01 reject pump->cpmm: fresh_net={:.6}", fresh_net);
    }

    let user_meme_ata = &simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let pump_data = pumpswap_section_data(&pool_meta, &pool)?;
    let cpmm_data = cpmm_section_data(&cpmm_state)?;

    let arb_ix = build_generic_route(
        wallet_pubkey, user_sol_ata, user_meme_ata, meme_mint, sol_mint,
        investment_lamports, min_meme_out, 1,
        constants::DEX_KIND_PUMPSWAP, constants::DEX_KIND_CPMM,
        None, None, Some(&pump_data), None,
        Some(&cpmm_data), None, None, None,
        pump_data.remaining_count, 0, true,
        meme_token_program, sol_token_program, config,
    ).await?;

    let ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_price(config.scanner.compute_unit_price_micro_lamports),
        ComputeBudgetInstruction::set_compute_unit_limit(config.scanner.compute_unit_limit),
        simulator::create_ata_idempotent_ix_v2(wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program),
        arb_ix,
    ];
    let alt_addr = Pubkey::from_str(config.execution_routing.onchain_arb_alt.as_deref().context("onchain_arb_alt")?)?;
    let alt = get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash).context("v0::Message")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet]).context("VersionedTransaction")?;
    log::info!("[ONCHAIN TX] pump->cpmm mint={} invest={:.6} est_meme={} est_sol={}",
        &opp.token_mint[..12.min(opp.token_mint.len())], opp.investment_sol, meme_out_est, sol_out_est);
    Ok((bincode::serialize(&tx)?, meme_out_est, sol_out_est))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_onchain_cpmm_to_pump_tx(
    opp: &ArbitrageOpportunity, wallet_pubkey: &Pubkey, wallet: &Keypair,
    config: &AppConfig, rpc: &RpcClient, sol_mint: &Pubkey, meme_mint: &Pubkey,
    token_program: &Pubkey, sol_token_program: &Pubkey,
    user_sol_ata: &Pubkey, _user_meme_ata: &Pubkey,
    investment_lamports: u64, blockhash: &solana_sdk::hash::Hash,
) -> anyhow::Result<(Vec<u8>, u64, u64)> {
    let meme_token_program = token_program;
    let cpmm_state = pool_cache::get_pool_state(&sol_mint.to_string(), &meme_mint.to_string())
        .or_else(|| pool_cache::get_pool_state_by_mint(&meme_mint.to_string()))
        .context("CPMM pool state not cached")?;
    let pool = pool_cache::resolve_pumpswap_pool_address(rpc, &meme_mint.to_string()).await
        .ok_or_else(|| anyhow::anyhow!("pool address not found"))?;
    let (pool_meta, fresh_sol_res, fresh_tok_res) =
        super::helpers::fetch_pumpswap_meta_and_reserves(rpc, &pool).await?;
    let cpmm_sol_vault = if cpmm_state.token_0_mint == sol_mint.to_string()
        { cpmm_state.token_0_vault_raw } else { cpmm_state.token_1_vault_raw };
    let cpmm_meme_vault = if cpmm_state.token_0_mint == sol_mint.to_string()
        { cpmm_state.token_1_vault_raw } else { cpmm_state.token_0_vault_raw };

    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);
    let meme_out_est = checked_cp_swap_output(cpmm_sol_vault, cpmm_meme_vault, investment_lamports, CPMM_DEFAULT_TRADE_FEE_RATE)
        .ok_or_else(|| anyhow::anyhow!("CPMM buy overflow"))?;
    let buy_slip = super::compute_effective_slippage(investment_lamports, cpmm_sol_vault, config.risk.slippage_tolerance_bps);
    let min_meme_out = (meme_out_est as f64 * buy_slip) as u64;

    let sol_out_est = simulator::checked_pumpswap_sell_output(
        min_meme_out, fresh_tok_res, fresh_sol_res, config.dex.pumpswap_fee_bps,
    ).ok_or_else(|| anyhow::anyhow!("PumpSwap sell overflow"))?;
    let fresh_net = sol_out_est as f64 / 1e9 - opp.investment_sol - cu_cost_sol;
    if fresh_net < config.risk.min_profit_threshold_sol {
        anyhow::bail!("R2-M01 reject cpmm->pump: fresh_net={:.6}", fresh_net);
    }

    let user_meme_ata = &simulator::ata_addr(wallet_pubkey, meme_mint, meme_token_program);
    let cpmm_data = cpmm_section_data(&cpmm_state)?;
    let pump_data = pumpswap_section_data(&pool_meta, &pool)?;

    let arb_ix = build_generic_route(
        wallet_pubkey, user_sol_ata, user_meme_ata, meme_mint, sol_mint,
        investment_lamports, min_meme_out, 1,
        constants::DEX_KIND_CPMM, constants::DEX_KIND_PUMPSWAP,
        Some(&cpmm_data), None, None, None,
        None, None, Some(&pump_data), None,
        0, pump_data.remaining_count, true,
        meme_token_program, sol_token_program, config,
    ).await?;

    let ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_price(config.scanner.compute_unit_price_micro_lamports),
        ComputeBudgetInstruction::set_compute_unit_limit(config.scanner.compute_unit_limit),
        simulator::create_ata_idempotent_ix_v2(wallet_pubkey, user_meme_ata, wallet_pubkey, meme_mint, meme_token_program),
        arb_ix,
    ];
    let alt_addr = Pubkey::from_str(config.execution_routing.onchain_arb_alt.as_deref().context("onchain_arb_alt")?)?;
    let alt = get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash).context("v0::Message")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet]).context("VersionedTransaction")?;
    log::info!("[ONCHAIN TX] cpmm->pump mint={} invest={:.6} est_meme={} est_sol={}",
        &opp.token_mint[..12.min(opp.token_mint.len())], opp.investment_sol, meme_out_est, sol_out_est);
    Ok((bincode::serialize(&tx)?, meme_out_est, sol_out_est))
}
