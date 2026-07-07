//! DLMM Buy -> PumpSwap AMM Sell (graduated token path)

use anyhow::Context;
use log::info;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::Instruction;
use solana_sdk::message::v0;
use solana_sdk::message::VersionedMessage;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::transaction::VersionedTransaction;
use std::str::FromStr;

use crate::arbitrage::ArbitrageOpportunity;
use crate::config::AppConfig;
use crate::pool_cache;
use crate::simulator;

use super::compute_effective_slippage;
use super::helpers::*;

#[allow(clippy::too_many_arguments, clippy::vec_init_then_push)]
pub(crate) async fn build_dlmm_buy_pumpswap_amm_sell(
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
    user_meme_ata: &Pubkey,
    investment_lamports: u64,
    blockhash: &solana_sdk::hash::Hash,
    bc: &pool_cache::BondingCurveState,
    dlmm: &pool_cache::DlmmPoolReserves,
) -> anyhow::Result<Vec<u8>> {
    let pool = Pubkey::from_str(&bc.bonding_curve_address).context("invalid pool address")?;

    // R2-M01: pool meta + fresh reserves + fresh DLMM bins in parallel
    let (pumpswap_result, fresh_bins_result) = tokio::join!(
        fetch_pumpswap_meta_and_reserves(rpc, &pool),
        pool_cache::fetch_bins_fresh(rpc, &dlmm.lb_pair),
    );
    let (pool_meta, fresh_sol_res, fresh_tok_res) = pumpswap_result?;
    let fresh_bins = fresh_bins_result?;

    // DLMM buy pricing
    let cu_cost_sol = super::compute_cu_cost_sol(&config.scanner);

    let sol_is_x = dlmm.token_x_mint == simulator::NATIVE_SOL_MINT;
    let (sol_reserve, _meme_reserve_for_buy) = if sol_is_x {
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
    let meme_out_est = simulator::estimate_dlmm_swap_output(
        &fresh_bins,
        investment_lamports,
        sol_is_x,
        dlmm_fee_rate,
    );
    let meme_out_est = if meme_out_est == 0 {
        anyhow::bail!(
            "DLMM bin estimate returned 0 — bins may be empty mint={}",
            &opp.token_mint[..12.min(opp.token_mint.len())]
        );
    } else {
        meme_out_est
    };
    let min_meme_out = (meme_out_est as f64 * buy_slippage) as u64;

    // PumpSwap AMM sell pricing with fresh reserves (R2-M01).
    // Conservative: use min_meme_out (guaranteed minimum from DLMM buy) as sell
    // amount to avoid insufficient balance.
    let sell_amount = min_meme_out;
    let sell_slippage = compute_effective_slippage(
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
    let min_sol_out = (sol_out_est as f64 * sell_slippage) as u64;

    // Re-estimate full route net profit with fresh PumpSwap reserves.
    // Reject if below absolute minimum or below 50% of the scanner's estimate.
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
    if fresh_net < opp.net_profit_sol * 0.95 {
        log::warn!(
            "[R2-M01 STALE] mint={} original_net={:.6} fresh_net={:.6} drop={:.1}%",
            &opp.token_mint[..12.min(opp.token_mint.len())],
            opp.net_profit_sol,
            fresh_net,
            (1.0 - fresh_net / opp.net_profit_sol.max(f64::EPSILON)) * 100.0,
        );
    }

    let mut ixs: Vec<Instruction> = Vec::new();

    ixs.push(ComputeBudgetInstruction::set_compute_unit_price(
        config.scanner.compute_unit_price_micro_lamports,
    ));
    ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(
        config.scanner.compute_unit_limit,
    ));

    ixs.push(simulator::create_ata_idempotent_ix_v2(
        wallet_pubkey,
        user_sol_ata,
        wallet_pubkey,
        sol_mint,
        sol_token_program,
    ));
    ixs.push(simulator::create_ata_idempotent_ix_v2(
        wallet_pubkey,
        user_meme_ata,
        wallet_pubkey,
        meme_mint,
        token_program,
    ));

    // Wrap SOL: transfer to WSOL ATA + sync_native before DLMM swap
    ixs.push(system_transfer_ix(
        wallet_pubkey,
        user_sol_ata,
        investment_lamports,
    ));
    ixs.push(sync_native_ix(user_sol_ata, sol_token_program));

    // DLMM Swap2: buy meme with SOL
    let lb_pair = Pubkey::from_str(&dlmm.lb_pair)?;
    let reserve_x = Pubkey::from_str(&dlmm.reserve_x_address)?;
    let reserve_y = Pubkey::from_str(&dlmm.reserve_y_address)?;
    let token_x_mint = Pubkey::from_str(&dlmm.token_x_mint)?;
    let token_y_mint = Pubkey::from_str(&dlmm.token_y_mint)?;
    let token_x_program = if dlmm.token_x_mint == simulator::NATIVE_SOL_MINT {
        sol_token_program
    } else {
        token_program
    };
    let token_y_program = if dlmm.token_y_mint == simulator::NATIVE_SOL_MINT {
        sol_token_program
    } else {
        token_program
    };
    let dlmm_program = Pubkey::from_str(simulator::DLMM_PROGRAM)?;
    let (oracle, _) =
        Pubkey::find_program_address(&[b"oracle", &lb_pair.to_bytes()], &dlmm_program);
    let memo_program = Pubkey::from_str(crate::constants::MEMO_PROGRAM)?;
    let event_auth = Pubkey::from_str(crate::constants::DLMM_EVENT_AUTHORITY)?;
    let bin_ext: Vec<Pubkey> = dlmm
        .bin_array_addresses
        .iter()
        .map(|a| Pubkey::from_str(a).unwrap())
        .collect();
    let bitmap_ext: Option<Pubkey> = dlmm
        .bin_array_bitmap_extension
        .as_deref()
        .and_then(|s| Pubkey::from_str(s).ok());

    ixs.push(simulator::build_dlmm_swap2_ix(
        wallet_pubkey,
        &lb_pair,
        &bin_ext,
        &reserve_x,
        &reserve_y,
        user_sol_ata,
        user_meme_ata,
        &token_x_mint,
        &token_y_mint,
        &oracle,
        &event_auth,
        investment_lamports,
        min_meme_out,
        token_x_program,
        token_y_program,
        &memo_program,
        &dlmm_program,
        bitmap_ext.as_ref(),
        None,
    ));

    // PumpSwap AMM sell: meme → SOL
    let buyback_recipient =
        Pubkey::from_str(crate::constants::PUMPSWAP_BUYBACK_FEE_RECIPIENT).unwrap();
    let protocol_fee_recipient = pick_pumpswap_protocol_fee_recipient(pool_meta.is_mayhem_mode);
    ixs.push(simulator::build_pumpswap_sell_ix(
        wallet_pubkey,
        &pool,
        meme_mint,
        sol_mint,
        user_meme_ata,
        user_sol_ata,
        &pool_meta.pool_base_token_account,
        &pool_meta.pool_quote_token_account,
        token_program,
        sol_token_program,
        sell_amount,
        min_sol_out,
        &pool_meta.coin_creator,
        pool_meta.is_cashback_coin,
        &buyback_recipient,
        &protocol_fee_recipient,
    ));

    // Close WSOL ATA to unwrap
    ixs.push(close_wsol_ata_ix(
        user_sol_ata,
        wallet_pubkey,
        sol_token_program,
    ));

    // ── v0 message with ALT (same ALT as on-chain path) ─
    let alt_addr = Pubkey::from_str(
        config
            .execution_routing
            .onchain_arb_alt
            .as_deref()
            .context("onchain_arb_alt not configured — cannot build v0 TX")?,
    )?;
    let alt = super::onchain_router::get_alt(rpc, alt_addr).await?;
    let v0_msg = v0::Message::try_compile(wallet_pubkey, &ixs, &[alt], *blockhash)
        .context("v0::Message::try_compile")?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet])
        .context("VersionedTransaction::try_new")?;

    info!(
        "[ATOMIC TX] dlmm→pumpswap mint={} invest={:.6} SOL est_meme={} est_sol_out={}",
        &opp.token_mint[..12.min(opp.token_mint.len())],
        opp.investment_sol,
        meme_out_est,
        sol_out_est,
    );

    bincode::serialize(&tx).context("serialize atomic tx")
}
