//! Integration test: residual_meme revert (ARB 6001).
//!
//! Uses a partial-sell PumpSwap stub that only sells 1 lamport of meme,
//! leaving the bulk of the DLMM-bought meme unsold. The route handler
//! must detect `meme_after != meme_before` and revert with ARB 6001.

mod common;
use common::*;

use solana_sdk::{
    compute_budget::ComputeBudgetInstruction,
    instruction::{AccountMeta, Instruction},
    signer::Signer,
    transaction::Transaction,
};

async fn do_residual_meme_test(amount_in: u64) -> u64 {
    let addrs = TestAddresses::new();
    // Deploy partial-sell stub at PUMP_SWAP_ID — sell leg only moves 1 lamport.
    let (mut ctx, account_order) =
        setup_dlmm_to_pump(&addrs, 1, amount_in, 0, STUB_PUMP_SWAP_PARTIAL_SO).await;

    // Net = 2×amount_in DLMM minus 1-lamport sell ≈ amount_in profit,
    // so min_profit=1 should still pass the profit check.
    let ix_data = build_ix_data(
        ROUTE_DLMM_TO_PUMP_DISC,
        amount_in,
        1, // min_profit_lamports — satisfied by 1-lamport sell
        1, // min_intermediate_meme
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
            let is_6001 = e.to_string().contains("custom program error: 0x1771"); // 0x1771 = 6001
            assert!(is_6001, "expected ARB 6001, got: {:?}", e);
        }
        Ok(()) => panic!("expected ARB 6001 but tx succeeded"),
    }

    cu
}

#[tokio::test]
async fn dlmm_to_pump_residual_meme() {
    let cu = do_residual_meme_test(1_000_000).await;
    eprintln!("dlmm_to_pump residual_meme: CU consumed = {}", cu);
}
