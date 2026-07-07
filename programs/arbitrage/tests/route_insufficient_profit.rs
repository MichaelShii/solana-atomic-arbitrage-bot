//! Integration test: insufficient_profit revert (ARB 6000).
//!
//! Sets min_profit_lamports higher than the stub rates can deliver
//! (DLMM 2x, PumpSwap buy 2x / sell 1x). Both routes must revert with
//! ProgramError::Custom(6000).

mod common;
use common::*;

use solana_program_test::ProgramTestContext;
use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    signer::Signer,
    transaction::Transaction,
};

// ── dlmm_to_pump: net = amount_in (2x DLMM × 1x PumpSwap sell) ────────

async fn do_dlmm_to_pump_insufficient_profit(amount_in: u64) -> (ProgramTestContext, u64) {
    let addrs = TestAddresses::new();
    let (mut ctx, account_order) =
        setup_dlmm_to_pump(&addrs, 1, amount_in, 0, STUB_PUMP_SWAP_SO).await;

    // Net profit = amount_in. Set min_profit > amount_in to trigger ARB 6000.
    let min_profit_lamports = amount_in + 1;

    let ix_data = build_ix_data(
        ROUTE_DLMM_TO_PUMP_DISC,
        amount_in,
        min_profit_lamports,
        1,
        false,
        false,
        0,
        1,
    );

    let accounts: Vec<AccountMeta> = account_order
        .iter()
        .enumerate()
        .map(|(i, pk)| {
            let is_signer = i == USER_IDX;
            let is_writable = matches!(
                i,
                USER_IDX
                    | USER_SOL_ATA_IDX
                    | USER_MEME_ATA_IDX
                    | 3
                    | 4
                    | 6
                    | 7
                    | 8
                    | 9
                    | 5
                    | 11
                    | 12
                    | 13
                    | 14
                    | 15
                    | 16
                    | 17
                    | 18
                    | 19
                    | 20
                    | 21
                    | 22
                    | 23
                    | 24
                    | 25
                    | 26
                    | 27
                    | 28
                    | 29
                    | 30
                    | 31
                    | 32
                    | 33
                    | 34
                    | 35
                    | 36
                    | 37
                    | 38
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

    let sim = ctx
        .banks_client
        .simulate_transaction(tx.clone())
        .await
        .unwrap();
    let cu = sim.simulation_details.unwrap().units_consumed;

    let result = ctx.banks_client.process_transaction(tx).await;
    match result {
        Err(e) => {
            let cu_val = e.to_string().contains("custom program error: 0x1770"); // 0x1770 = 6000
            assert!(cu_val, "expected ARB 6000, got: {:?}", e);
        }
        Ok(()) => panic!("expected ARB 6000 but tx succeeded"),
    }

    (ctx, cu)
}

// ── pump_to_dlmm: net = 3×amount_in (2x PumpSwap buy × 2x DLMM) ───────

async fn do_pump_to_dlmm_insufficient_profit(amount_in: u64) -> (ProgramTestContext, u64) {
    let addrs = TestAddresses::new();
    let (mut ctx, account_order) = setup_pump_to_dlmm(&addrs, 1, amount_in, 0).await;

    // Net profit = 3×amount_in. Set min_profit > 3×amount_in to trigger ARB 6000.
    let min_profit_lamports = amount_in.saturating_mul(3).saturating_add(1);

    let ix_data = build_ix_data(
        ROUTE_PUMP_TO_DLMM_DISC,
        amount_in,
        min_profit_lamports,
        1,
        false,
        false,
        0,
        1,
    );

    let accounts: Vec<AccountMeta> = account_order
        .iter()
        .enumerate()
        .map(|(i, pk)| {
            let is_signer = i == USER_IDX;
            let is_writable = matches!(
                i,
                USER_IDX
                    | USER_SOL_ATA_IDX
                    | USER_MEME_ATA_IDX
                    | 3   // PUMP_BUY_POOL (writable per pump_swap::build_buy)
                    | 4
                    | 8
                    | 9
                    | 10
                    | 11
                    | 13
                    | 20
                    | 22
                    | 23
                    | 26
                    | 27
                    | 28  // DLMM bitmap (writable since DLMM v0.12.0)
                    | 29
                    | 30
                    | 31
                    | 32
                    | 34
                    | 35
                    | 36
                    | 37
                    | 38
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

    let sim = ctx
        .banks_client
        .simulate_transaction(tx.clone())
        .await
        .unwrap();
    let cu = sim.simulation_details.unwrap().units_consumed;

    let result = ctx.banks_client.process_transaction(tx).await;
    match result {
        Err(e) => {
            let cu_val = e.to_string().contains("custom program error: 0x1770");
            assert!(cu_val, "expected ARB 6000, got: {:?}", e);
        }
        Ok(()) => panic!("expected ARB 6000 but tx succeeded"),
    }

    (ctx, cu)
}

#[tokio::test]
async fn dlmm_to_pump_insufficient_profit() {
    let (_ctx, cu) = do_dlmm_to_pump_insufficient_profit(1_000_000).await;
    eprintln!("dlmm_to_pump insufficient_profit: CU consumed = {}", cu);
}

#[tokio::test]
async fn pump_to_dlmm_insufficient_profit() {
    let (_ctx, cu) = do_pump_to_dlmm_insufficient_profit(1_000_000).await;
    eprintln!("pump_to_dlmm insufficient_profit: CU consumed = {}", cu);
}
