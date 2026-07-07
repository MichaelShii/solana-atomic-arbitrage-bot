//! Integration test: route_dlmm_to_pump happy path.
//!
//! DLMM swap2 (SOL -> meme, 2x rate stub) -> PumpSwap sell (meme -> SOL, 1x rate stub).
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

async fn do_dlmm_to_pump_test(
    amount_in: u64,
    min_profit_lamports: u64,
) -> (ProgramTestContext, u64) {
    let addrs = TestAddresses::new();
    let (mut ctx, account_order) =
        setup_dlmm_to_pump(&addrs, 1, amount_in, 0, STUB_PUMP_SWAP_SO).await;

    let ix_data = build_ix_data(
        ROUTE_DLMM_TO_PUMP_DISC,
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
                    // DLMM writable: program(rel0), lb_pair(rel1), reserve_x(rel3), reserve_y(rel4),
                    // oracle(rel5), host_fee(rel6), user_token_in/out, bin_arrays
                    | 3 | 4 | 6 | 7 | 8 | 9
                    | 5 // user_token_in (idx 5 of DLMM section = WRITABLE when in swap2)
                    | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 | 22
                    | 23 | 24 | 25 | 26 | 27 | 28 | 29 | 30 | 31 | 32
                    | 33 | 34 | 35 | 36 | 37 | 38
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

    // Clone tx for simulation (get CU before process_transaction consumes tx)
    let sim_tx = tx.clone();
    let sim = ctx.banks_client.simulate_transaction(sim_tx).await.unwrap();
    let cu = sim.simulation_details.unwrap().units_consumed;

    let result = ctx.banks_client.process_transaction(tx).await;
    assert!(
        result.is_ok(),
        "route_dlmm_to_pump failed: {:?}",
        result.err()
    );
    assert!(cu <= 180_000, "CU {} exceeds 180k limit", cu);

    (ctx, cu)
}

#[tokio::test]
async fn dlmm_to_pump_happy_path_small() {
    let (_ctx, cu) = do_dlmm_to_pump_test(1_000_000, 1).await;
    eprintln!("dlmm_to_pump small: CU consumed = {}", cu);
}

#[tokio::test]
async fn dlmm_to_pump_happy_path_large() {
    let (_ctx, cu) = do_dlmm_to_pump_test(100_000_000, 1).await;
    eprintln!("dlmm_to_pump large: CU consumed = {}", cu);
}
