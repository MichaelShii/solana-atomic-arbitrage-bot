//! Integration test: route_pump_to_dlmm happy path.
//!
//! PumpSwap buy (SOL -> meme, 2x rate stub) -> DLMM swap2 (meme -> SOL, 2x rate stub).
//! Verifies the instruction succeeds and CU <= 180k.

mod common;
use common::*;

use solana_program_test::ProgramTestContext;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    signer::Signer,
    transaction::Transaction,
};

async fn do_pump_to_dlmm_test(
    amount_in: u64,
    min_profit_lamports: u64,
) -> (ProgramTestContext, u64) {
    let addrs = TestAddresses::new();
    let (mut ctx, account_order) = setup_pump_to_dlmm(&addrs, 1, amount_in, 0).await;

    let ix_data = build_ix_data(
        ROUTE_PUMP_TO_DLMM_DISC,
        amount_in,
        min_profit_lamports,
        1,     // min_intermediate_meme
        false, // track_volume
        false, // dlmm_sol_is_x
        0,     // pump_remaining_count
        1,     // dlmm_bin_array_count
    );

    let accounts: Vec<AccountMeta> = account_order
        .iter()
        .enumerate()
        .map(|(i, pk)| {
            let is_signer = i == USER_IDX;
            let is_writable = matches!(
                i,
                USER_IDX | USER_SOL_ATA_IDX | USER_MEME_ATA_IDX
                    // PumpSwap Buy writable
                    | 3  // pool
                    | 8 | 9 | 10 | 11 | 13  // ATAs + fee
                    | 20 | 22 | 23 // vol_accum, coin_creator_vault_ata, *
                    // DLMM writable
                    | 26 | 27 | 28 | 29 | 30 | 31 | 32
                    | 34 | 35 | 36 | 37 | 38
            );
            AccountMeta {
                pubkey: *pk,
                is_signer,
                is_writable,
            }
        })
        .collect();

    let ix = Instruction::new_with_bytes(ARBITRAGE_ID, &ix_data, accounts);

    let cu_limit_ix = ComputeBudgetInstruction::set_compute_unit_limit(200_000);
    let tx = Transaction::new_signed_with_payer(
        &[cu_limit_ix, ix],
        Some(&addrs.user.pubkey()),
        &[&addrs.user],
        ctx.last_blockhash,
    );

    // Simulate to get CU
    let sim_tx = tx.clone();
    let sim = ctx.banks_client.simulate_transaction(sim_tx).await.unwrap();
    let cu = sim.simulation_details.unwrap().units_consumed;

    let result = ctx.banks_client.process_transaction(tx).await;
    assert!(
        result.is_ok(),
        "route_pump_to_dlmm failed: {:?}",
        result.err()
    );
    assert!(cu <= 180_000, "CU {} exceeds 180k limit", cu);

    (ctx, cu)
}

#[tokio::test]
async fn pump_to_dlmm_happy_path_small() {
    let (_ctx, cu) = do_pump_to_dlmm_test(1_000_000, 1).await;
    eprintln!("pump_to_dlmm small: CU consumed = {}", cu);
}

#[tokio::test]
async fn pump_to_dlmm_happy_path_large() {
    let (_ctx, cu) = do_pump_to_dlmm_test(100_000_000, 1).await;
    eprintln!("pump_to_dlmm large: CU consumed = {}", cu);
}
