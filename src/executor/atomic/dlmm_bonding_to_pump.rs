//! DLMM Buy -> PumpSwap Bonding Curve Sell

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

use super::{compute_effective_slippage, resolve_pumpfun_fee_recipient};

#[allow(clippy::too_many_arguments, clippy::vec_init_then_push)]
pub(crate) async fn build_dlmm_buy_pumpswap_sell(
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
) -> anyhow::Result<Vec<u8>> {
    let dlmm = pool_cache::get_dlmm_reserves(simulator::NATIVE_SOL_MINT, &opp.token_mint)
        .context("DLMM reserves not cached")?;

    // Fetch bonding curve + fresh DLMM bins in parallel
    let (bc_result, fresh_bins_result) = tokio::join!(
        pool_cache::fetch_bonding_curve(rpc, &opp.token_mint),
        pool_cache::fetch_bins_fresh(rpc, &dlmm.lb_pair),
    );
    let bc = bc_result.context("bonding curve not found")?;
    let fresh_bins = fresh_bins_result?;

    if bc.venue_kind == pool_cache::PumpVenueKind::PumpSwapPool {
        return super::dlmm_amm_to_pump::build_dlmm_buy_pumpswap_amm_sell(
            opp,
            wallet_pubkey,
            wallet,
            config,
            rpc,
            sol_mint,
            meme_mint,
            token_program,
            sol_token_program,
            user_sol_ata,
            user_meme_ata,
            investment_lamports,
            blockhash,
            &bc,
            &dlmm,
        )
        .await;
    }

    let bonding_curve = Pubkey::from_str(&bc.bonding_curve_address)?;
    let bc_meme_ata = simulator::ata_addr(&bonding_curve, meme_mint, token_program);
    let fee_recipient = resolve_pumpfun_fee_recipient(rpc, config).await;

    let sol_is_x = dlmm.token_x_mint == simulator::NATIVE_SOL_MINT;
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
    let meme_out_est = simulator::estimate_dlmm_swap_output(
        &fresh_bins,
        investment_lamports,
        sol_is_x,
        dlmm_fee_rate,
    );
    let meme_out_est = if meme_out_est == 0 {
        simulator::estimate_swap_output(
            sol_reserve,
            meme_reserve_for_buy,
            investment_lamports,
            dlmm_fee_rate,
        )
    } else {
        meme_out_est
    };
    let min_meme_out = (meme_out_est as f64 * buy_slippage) as u64;

    let sell_slippage = compute_effective_slippage(
        meme_out_est,
        bc.virtual_token_reserves,
        config.risk.slippage_tolerance_bps,
    );
    let sol_out_est = simulator::estimate_pumpfun_sell_output(
        meme_out_est,
        bc.virtual_sol_reserves,
        bc.virtual_token_reserves,
        config.dex.pumpfun_fee_bps,
    );
    let min_sol_out = (sol_out_est as f64 * sell_slippage) as u64;

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

    let lb_pair = Pubkey::from_str(&dlmm.lb_pair)?;
    let reserve_x = Pubkey::from_str(&dlmm.reserve_x_address)?;
    let reserve_y = Pubkey::from_str(&dlmm.reserve_y_address)?;
    // token_x/y_mint and their programs MUST match the DLMM pool's actual X/Y config,
    // NOT the swap direction — the program validates against stored mint order.
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

    ixs.push(simulator::build_pumpfun_sell_ix(
        wallet_pubkey,
        meme_mint,
        &bonding_curve,
        &bc_meme_ata,
        user_meme_ata,
        &fee_recipient,
        meme_out_est,
        min_sol_out,
        token_program,
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
        "[ATOMIC TX] dlmm→pumpfun mint={} invest={:.6} SOL est_meme={} est_sol_out={}",
        &opp.token_mint[..12.min(opp.token_mint.len())],
        opp.investment_sol,
        meme_out_est,
        sol_out_est,
    );

    bincode::serialize(&tx).context("serialize atomic tx")
}
