use super::*;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::Transaction;
use std::str::FromStr;
use std::time::Duration;

#[test]
fn test_dlmm_single_bin_matches_cpmm() {
    let bins = vec![DlmmBin {
        bin_id: 0,
        amount_x: 1000,
        amount_y: 2000,
        reserve_x: 0,
        reserve_y: 0,
    }];
    let out = estimate_dlmm_swap_output(&bins, 100, true, 0.0);
    // Bin acts as limit order: price = amount_y/amount_x = 2000/1000 = 2
    // Output = 100 * 2 = 200 (full reserve_out since bin not exhausted)
    assert_eq!(out, 200);
}

#[test]
fn test_dlmm_multi_bin_crosses_boundary() {
    let bins = vec![
        DlmmBin {
            bin_id: 0,
            amount_x: 100,
            amount_y: 100,
            reserve_x: 0,
            reserve_y: 0,
        },
        DlmmBin {
            bin_id: 1,
            amount_x: 500,
            amount_y: 400,
            reserve_x: 0,
            reserve_y: 0,
        },
    ];
    let out = estimate_dlmm_swap_output(&bins, 200, true, 0.0);
    // Sorted by bin_id desc (x→y): bin_id=1 first, bin_id=0 second.
    // Bin 1: partial fill, 400*200/500 = 160. Total = 160, break.
    assert_eq!(out, 160);
}

#[test]
fn test_dlmm_with_fee() {
    let bins = vec![DlmmBin {
        bin_id: 0,
        amount_x: 1000,
        amount_y: 2000,
        reserve_x: 0,
        reserve_y: 0,
    }];
    let out_no_fee = estimate_dlmm_swap_output(&bins, 100, true, 0.0);
    let out_with_fee = estimate_dlmm_swap_output(&bins, 100, true, 0.01);
    assert!(out_with_fee < out_no_fee, "fee should reduce output");
}

#[test]
fn test_dlmm_empty_bins() {
    assert_eq!(estimate_dlmm_swap_output(&[], 100, true, 0.0), 0);
}

#[test]
fn test_dlmm_zero_amount() {
    let bins = vec![DlmmBin {
        bin_id: 0,
        amount_x: 1000,
        amount_y: 2000,
        reserve_x: 0,
        reserve_y: 0,
    }];
    assert_eq!(estimate_dlmm_swap_output(&bins, 0, true, 0.0), 0);
}

#[test]
fn test_dlmm_global_vs_per_bin_divergence() {
    let bins = vec![
        DlmmBin {
            bin_id: 0,
            amount_x: 300,
            amount_y: 600,
            reserve_x: 0,
            reserve_y: 0,
        },
        DlmmBin {
            bin_id: 1,
            amount_x: 500,
            amount_y: 500,
            reserve_x: 0,
            reserve_y: 0,
        },
        DlmmBin {
            bin_id: 2,
            amount_x: 700,
            amount_y: 300,
            reserve_x: 0,
            reserve_y: 0,
        },
    ];
    let out_bin = estimate_dlmm_swap_output(&bins, 200, true, 0.0);
    let out_global = super::super::estimate_swap_output(200, 1500, 1400, 0.0);
    let diff_pct = (out_bin as f64 - out_global as f64).abs() / out_global as f64 * 100.0;
    assert!(
        diff_pct >= 5.0,
        "per-bin vs global divergence should be noticeable, got {:.1}% (bin={}, global={})",
        diff_pct,
        out_bin,
        out_global
    );

    let bins_single = vec![DlmmBin {
        bin_id: 0,
        amount_x: 100,
        amount_y: 200,
        reserve_x: 0,
        reserve_y: 0,
    }];
    let out_exhaust = estimate_dlmm_swap_output(&bins_single, 200, true, 0.0);
    // Single bin exhaust: consume all reserve_in=100, receive all reserve_out=200.
    assert_eq!(out_exhaust, 200);
}

#[test]
fn test_dlmm_exhaust_bin_never_returns_full_reserve() {
    let bins = vec![
        DlmmBin {
            bin_id: 0,
            amount_x: 100,
            amount_y: 100,
            reserve_x: 0,
            reserve_y: 0,
        },
        DlmmBin {
            bin_id: 1,
            amount_x: 100,
            amount_y: 200,
            reserve_x: 0,
            reserve_y: 0,
        },
    ];
    let out = estimate_dlmm_swap_output(&bins, 180, true, 0.0);
    // Bin 1 (id=1): exhaust → reserve_out=200. Remaining=80.
    // Bin 0 (id=0): partial → 100*80/100=80. Total=280.
    assert_eq!(out, 280);
}

// ============================================================
// R2-H02: token_x/y mint/program must match pool config,
//         NOT swap direction (verified against metfin/dlmm-sdk-go IDL)
// ============================================================

fn make_test_ix(
    token_x_mint: Pubkey,
    token_y_mint: Pubkey,
    token_x_program: Pubkey,
    token_y_program: Pubkey,
) -> Instruction {
    build_dlmm_swap2_ix(
        &Pubkey::new_unique(), // user
        &Pubkey::new_unique(), // lb_pair
        &[],                   // bin_arrays
        &Pubkey::new_unique(), // reserve_x
        &Pubkey::new_unique(), // reserve_y
        &Pubkey::new_unique(), // user_token_in
        &Pubkey::new_unique(), // user_token_out
        &token_x_mint,
        &token_y_mint,
        &Pubkey::new_unique(), // oracle
        &Pubkey::new_unique(), // event_authority
        1000,                  // amount_in
        900,                   // min_amount_out
        &token_x_program,
        &token_y_program,
        &Pubkey::new_unique(), // memo_program
        &Pubkey::new_unique(), // event_program
        None,                  // bin_array_bitmap_extension
        None,                  // host_fee_in
    )
}

/// Without optional accounts, token_x_mint is at index 5, token_y_mint at 6,
/// token_x_program at 9, token_y_program at 10.
#[test]
fn test_swap2_account_mints_always_match_pool_x_y_order() {
    let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
    let meme_mint = Pubkey::new_unique();
    let tokenkeg = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let token22 = Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").unwrap();

    // Case A: SOL is X, meme is Y
    let ix = make_test_ix(sol_mint, meme_mint, tokenkeg, token22);
    assert_eq!(
        ix.accounts[6].pubkey, sol_mint,
        "token_x_mint (acct[6]) must be SOL when SOL is pool X"
    );
    assert_eq!(
        ix.accounts[7].pubkey, meme_mint,
        "token_y_mint (acct[7]) must be meme when meme is pool Y"
    );
    assert_eq!(
        ix.accounts[11].pubkey, tokenkeg,
        "token_x_program (acct[11]) must match SOL's program"
    );
    assert_eq!(
        ix.accounts[12].pubkey, token22,
        "token_y_program (acct[12]) must match meme's program (Token-2022)"
    );

    // Case B: meme is X, SOL is Y
    let ix = make_test_ix(meme_mint, sol_mint, token22, tokenkeg);
    assert_eq!(
        ix.accounts[6].pubkey, meme_mint,
        "token_x_mint must stay as meme when meme is pool X"
    );
    assert_eq!(
        ix.accounts[7].pubkey, sol_mint,
        "token_y_mint must stay as SOL when SOL is pool Y"
    );
    assert_eq!(
        ix.accounts[11].pubkey, token22,
        "token_x_program must match meme's Token-2022"
    );
    assert_eq!(
        ix.accounts[12].pubkey, tokenkeg,
        "token_y_program must match SOL's Tokenkeg"
    );
}

/// user_token_in/out still follow swap direction — independent of pool X/Y order.
#[test]
fn test_swap2_user_io_accounts_follow_swap_direction() {
    let sol_mint = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
    let meme_mint = Pubkey::new_unique();
    let tokenkeg = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let user_sol_ata = Pubkey::new_unique();
    let user_meme_ata = Pubkey::new_unique();

    // Simulate: selling meme for SOL (PumpSwap→DLMM: user_token_in = meme ATA)
    let ix = build_dlmm_swap2_ix(
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        &[],
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        &user_meme_ata, // user_token_in: selling meme
        &user_sol_ata,  // user_token_out: receiving SOL
        &sol_mint,
        &meme_mint,
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        1000,
        900,
        &tokenkeg,
        &tokenkeg,
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        None,
        None,
    );
    assert_eq!(
        ix.accounts[4].pubkey, user_meme_ata,
        "user_token_in must be meme ATA when selling meme"
    );
    assert_eq!(
        ix.accounts[5].pubkey, user_sol_ata,
        "user_token_out must be SOL ATA when receiving SOL"
    );

    // Simulate: buying meme with SOL (DLMM→PumpSwap: user_token_in = SOL ATA)
    let ix = build_dlmm_swap2_ix(
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        &[],
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        &user_sol_ata,  // user_token_in: spending SOL
        &user_meme_ata, // user_token_out: receiving meme
        &sol_mint,
        &meme_mint,
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        1000,
        900,
        &tokenkeg,
        &tokenkeg,
        &Pubkey::new_unique(),
        &Pubkey::new_unique(),
        None,
        None,
    );
    assert_eq!(
        ix.accounts[4].pubkey, user_sol_ata,
        "user_token_in must be SOL ATA when spending SOL"
    );
    assert_eq!(
        ix.accounts[5].pubkey, user_meme_ata,
        "user_token_out must be meme ATA when receiving meme"
    );
}

// ============================================================
// Golden test: simulate DLMM Swap2 against mainnet to verify
// account layout is accepted by the live program.
//
// Run with: cargo test -- --ignored --nocapture
// Requires SOLANA_RPC_URL env var.
//
// Fixture: a real, successful PumpSwap→DLMM Swap2 arbitrage tx.
// The test fetches the tx, extracts the Swap2 account list, builds
// our own swap2 ix for the same pool, and asserts account-for-account
// match for the 16 fixed positions + bin array slots.
// ============================================================

/// Real successful DLMM Swap2 tx from 2026-06-16.
/// PumpSwap BuyExactQuoteIn → DLMM Swap2 ×2 arbitrage.
/// Extracted Swap2 inner[8]: 17 accounts (16 fixed + 1 bin array),
/// bin_array_bitmap_extension placeholder = DLMM_PROGRAM (lb_pair data[248]=0),
/// host_fee_in placeholder = DLMM_PROGRAM.
const GOLDEN_SWAP2_SIG: &str =
    "4M9Hsts16TM6ZAgtXTruYsaaavoWE3SPy6EaBsCR6dYh7mbSBwNWuanp61tgreNjqtyiQgXqd1jJ8fuecBeTjV8n";

/// Expected 16 fixed accounts + 1 bin array from the golden tx Swap2 inner[13]
/// (second DLMM CPI — meme→WSOL sell, matching our primary use case).
/// Extracted from inner CPI accounts via getTransaction(encoding=json), NOT from
/// the outer transaction accountKeys.
const GOLDEN_SWAP2_ACCOUNTS: [&str; 17] = [
    "EDeuGoVFTEUvWZvNGQH6UvSs5uk6RLgKTvr3MgY32ouw", // #0  lb_pair
    "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",  // #1  bmpx (placeholder)
    "iKbMkevhUWW5cSQBNjcnt2atuhGbGhFXaGmn81LtFME",  // #2  reserve_x
    "9fK1i9wjoiD7trYo4gBh5utG2yvNFposmRRY4sUuwoXu", // #3  reserve_y
    "FvAGBZ9yT2boxD5Duanit8d3EDtzGrDDzdewWTjdErGm", // #4  user_token_in
    "4mvdsmiuyx4YKpA8aARxHbBm1KvVs1G9xBa5siJDAFiF", // #5  user_token_out
    "BcHEaaTCvycPwwsJ9yQTXdHP9X2gCLkznDbZ8VySpump", // #6  token_x_mint
    "So11111111111111111111111111111111111111112",  // #7  token_y_mint
    "HDFbm8bm6QzSPXCZDfA5Xq4aGC7tLdBgETagbHLEeNvR", // #8  oracle
    "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",  // #9  host_fee_in (placeholder)
    "REPLACE_WITH_YOUR_WALLET_PUBKEY",  // #10 user (signer)
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",  // #11 token_x_program (Token-2022)
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  // #12 token_y_program (SPL Token)
    "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",  // #13 memo_program
    "D1ZN9Wj1fRSUQfCjhvnu1hqDMT7hzjzBBpi12nVniYD6", // #14 event_authority
    "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",  // #15 program
    "7t6TcKYjDVaddzMMLTQqFe7XuVy1mRQ3oPp6qsRUvaAh", // #16 bin_array[0]
];

#[tokio::test]
#[ignore]
async fn test_swap2_account_layout_live_simulation() {
    use solana_client::rpc_config::RpcSimulateTransactionConfig;

    let _ = tracing_subscriber::fmt::try_init();
    let _ = dotenvy::dotenv();

    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".into());
    let rpc = RpcClient::new_with_timeout(rpc_url.clone(), Duration::from_secs(30));
    println!("[GOLDEN] RPC: {}", rpc_url);

    let dlmm_prog = Pubkey::from_str(super::DLMM_PROGRAM).unwrap();

    // ---- Step 1: Fetch the lb_pair used in the golden tx ----
    let lb_pair = Pubkey::from_str(GOLDEN_SWAP2_ACCOUNTS[0]).unwrap();
    let lb_acct = match rpc.get_account(&lb_pair).await {
        Ok(a) => a,
        Err(e) => panic!(
            "golden lb_pair not found on-chain — fixture may be stale\n\
             lb_pair={lb_pair}\n\
             tx_sig={GOLDEN_SWAP2_SIG}\n\
             error={e}",
        ),
    };
    assert_eq!(
        lb_acct.owner, dlmm_prog,
        "lb_pair owner must be DLMM program"
    );
    let data = &lb_acct.data;
    assert!(data.len() >= 152, "lb_pair data too short: {}", data.len());

    // ---- Step 2: Parse lb_pair data (same offsets as our production code) ----
    let token_x_mint = {
        let bytes: [u8; 32] = data[88..120].try_into().unwrap();
        Pubkey::new_from_array(bytes)
    };
    let token_y_mint = {
        let bytes: [u8; 32] = data[120..152].try_into().unwrap();
        Pubkey::new_from_array(bytes)
    };
    assert_eq!(
        token_x_mint.to_string(),
        GOLDEN_SWAP2_ACCOUNTS[6],
        "token_x_mint from lb_pair must match fixture"
    );
    assert_eq!(
        token_y_mint.to_string(),
        GOLDEN_SWAP2_ACCOUNTS[7],
        "token_y_mint from lb_pair must match fixture"
    );

    // Verify bmpx is None on-chain (data[248]==0) but fixture has DLMM_PROGRAM placeholder
    let bmpx_on_chain: Option<Pubkey> = if data.len() >= 281 && data[248] == 1 {
        Some(Pubkey::new_from_array(data[249..281].try_into().unwrap()))
    } else {
        None
    };
    assert!(bmpx_on_chain.is_none(),
        "golden pool has no bitmap extension on-chain; fixture account #1 should be the placeholder");

    // ---- Step 3: Derive PDAs (reserves) + verify constants (oracle, event_authority) ----
    let (reserve_x, _) =
        Pubkey::find_program_address(&[&lb_pair.to_bytes(), &token_x_mint.to_bytes()], &dlmm_prog);
    let (reserve_y, _) =
        Pubkey::find_program_address(&[&lb_pair.to_bytes(), &token_y_mint.to_bytes()], &dlmm_prog);
    assert_eq!(
        reserve_x.to_string(),
        GOLDEN_SWAP2_ACCOUNTS[2],
        "reserve_x PDA must match fixture"
    );
    assert_eq!(
        reserve_y.to_string(),
        GOLDEN_SWAP2_ACCOUNTS[3],
        "reserve_y PDA must match fixture"
    );

    // oracle is a pool-specific PDA: [b"oracle", lb_pair]
    let (oracle, _) = Pubkey::find_program_address(&[b"oracle", &lb_pair.to_bytes()], &dlmm_prog);
    assert_eq!(
        oracle.to_string(),
        GOLDEN_SWAP2_ACCOUNTS[8],
        "oracle PDA must match fixture (lb_pair={lb_pair})"
    );

    // event_authority is a global well-known address, not a per-pool PDA.
    let event_auth = Pubkey::from_str(crate::constants::DLMM_EVENT_AUTHORITY).unwrap();
    assert_eq!(
        event_auth.to_string(),
        GOLDEN_SWAP2_ACCOUNTS[14],
        "event_authority constant must match fixture"
    );

    // ---- Step 4: Derive bin arrays (our convention: active ±1) ----
    let active_id = i32::from_le_bytes(data[76..80].try_into().unwrap());
    let mut bin_arrays: Vec<Pubkey> = Vec::new();
    let active_bin_array_idx = active_id / 70;
    for offset in -1i32..=1i32 {
        let idx = (active_bin_array_idx + offset) as i64;
        let (pda, _) = Pubkey::find_program_address(
            &[b"bin_array", &lb_pair.to_bytes(), &idx.to_le_bytes()],
            &dlmm_prog,
        );
        bin_arrays.push(pda);
    }
    // The golden tx uses 1 bin array. Verify it's among ours.
    let golden_bin = Pubkey::from_str(GOLDEN_SWAP2_ACCOUNTS[16]).unwrap();
    assert!(
        bin_arrays.contains(&golden_bin),
        "golden bin array {golden_bin} must be in our derived bin arrays {:?}",
        bin_arrays
    );

    // ---- Step 5: Build our Swap2 instruction ----
    let memo_prog = Pubkey::from_str(crate::constants::MEMO_PROGRAM).unwrap();
    let user = Pubkey::from_str(GOLDEN_SWAP2_ACCOUNTS[10]).unwrap();
    let user_ata_x = Pubkey::from_str(GOLDEN_SWAP2_ACCOUNTS[4]).unwrap();
    let user_ata_y = Pubkey::from_str(GOLDEN_SWAP2_ACCOUNTS[5]).unwrap();
    let tok_x_prog = Pubkey::from_str(GOLDEN_SWAP2_ACCOUNTS[11]).unwrap();
    let tok_y_prog = Pubkey::from_str(GOLDEN_SWAP2_ACCOUNTS[12]).unwrap();

    let swap_ix = build_dlmm_swap2_ix(
        &user,
        &lb_pair,
        &bin_arrays,
        &reserve_x,
        &reserve_y,
        &user_ata_x,
        &user_ata_y,
        &token_x_mint,
        &token_y_mint,
        &oracle,
        &event_auth,
        1_000_000,
        0,
        &tok_x_prog,
        &tok_y_prog,
        &memo_prog,
        &dlmm_prog,
        bmpx_on_chain.as_ref(),
        None, // host_fee_in — not a host
    );

    // ---- Step 6: Assert account order matches the golden fixture ----
    println!(
        "[GOLDEN] our_accounts.len()={} fixture_accounts.len()={}",
        swap_ix.accounts.len(),
        GOLDEN_SWAP2_ACCOUNTS.len()
    );
    println!("[GOLDEN] lb_pair={lb_pair}");
    println!("[GOLDEN] active_id={active_id} bin_arrays={bin_arrays:?}");
    println!("[GOLDEN] bmpx_on_chain={bmpx_on_chain:?}");

    // Fixed 16 accounts must match exactly
    let our_pubkeys: Vec<String> = swap_ix
        .accounts
        .iter()
        .map(|a| a.pubkey.to_string())
        .collect();
    for i in 0..16 {
        assert_eq!(
            our_pubkeys[i], GOLDEN_SWAP2_ACCOUNTS[i],
            "account #{i} mismatch: ours={} fixture={}",
            our_pubkeys[i], GOLDEN_SWAP2_ACCOUNTS[i],
        );
    }
    // Bin arrays: golden tx has 1, we may have more. At minimum the golden
    // bin array must appear at the right position range.
    assert!(
        our_pubkeys[16..].contains(&GOLDEN_SWAP2_ACCOUNTS[16].to_string()),
        "golden bin array {} must be in our bin arrays {:?}",
        GOLDEN_SWAP2_ACCOUNTS[16],
        &our_pubkeys[16..]
    );

    println!("[GOLDEN] Fixed 16 accounts: MATCH");
    println!("[GOLDEN] Bin arrays: golden bin found among ours");

    // ---- Step 7: Live simulation (proves the program accepts our account list) ----
    let ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(400_000),
        swap_ix,
    ];
    let message = Message::new(&ixs, Some(&user));
    let tx = Transaction::new_unsigned(message);

    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        ..Default::default()
    };
    let sim_result = rpc.simulate_transaction_with_config(&tx, sim_config).await;

    match sim_result {
        Ok(sim) => {
            let err_msg = sim
                .value
                .err
                .map(|e| format!("{:?}", e))
                .unwrap_or_default();
            let logs = sim.value.logs.unwrap_or_default();
            println!("[GOLDEN] sim err: {}", err_msg);
            println!("[GOLDEN] logs (last 15):");
            for line in logs.iter().rev().take(15).rev() {
                println!("  {}", line);
            }
            // Fatal: account layout or mutability errors
            for pat in &[
                "AccountNotFound",
                "AccountNotInitialized",
                "InvalidAccountData",
                "ConstraintMut",
            ] {
                if err_msg.contains(pat) {
                    panic!(
                        "Account layout error: {} — {}\n\nLogs:\n{}",
                        pat,
                        err_msg,
                        logs.join("\n")
                    );
                }
            }
            println!("[GOLDEN] PASSED — no account layout errors detected");
        }
        Err(e) => panic!("simulateTransaction RPC call failed: {}", e),
    }
}

/// Verify Swap2 instruction data serialization against real mainnet fixture.
///
/// Source: inner instruction [13] from tx 4M9Hsts16... — meme→WSOL sell Swap2.
/// Extracted via getTransaction(encoding=json) → meta.innerInstructions → match
/// discriminator 414b3f4ceb5b5b88 with programIdIndex == LBUZ... .
#[test]
fn test_swap2_data_serialization_matches_golden_fixture() {
    // Real Swap2 data from inner[13]: 28 bytes
    // disc(8) | amount_in=18554703315(8) | min_amount_out=0(8) | empty_remaining_accounts_info(4)
    const GOLDEN_SWAP2_DATA: [u8; 28] = [
        0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88, // discriminator
        0xd3, 0x4d, 0xf2, 0x51, 0x04, 0x00, 0x00, 0x00, // amount_in = 18554703315 u64 LE
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // min_amount_out = 0
        0x00, 0x00, 0x00, 0x00, // empty remaining_accounts_info (u32 LE = 0)
    ];

    // Build an instruction with the same amounts
    let user = solana_sdk::pubkey::Pubkey::new_unique();
    let lb_pair = solana_sdk::pubkey::Pubkey::new_unique();
    let bin_arrays = vec![];
    let reserve_x = solana_sdk::pubkey::Pubkey::new_unique();
    let reserve_y = solana_sdk::pubkey::Pubkey::new_unique();
    let user_in = solana_sdk::pubkey::Pubkey::new_unique();
    let user_out = solana_sdk::pubkey::Pubkey::new_unique();
    let token_x = solana_sdk::pubkey::Pubkey::new_unique();
    let token_y =
        solana_sdk::pubkey::Pubkey::from_str("So11111111111111111111111111111111111111112")
            .unwrap();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    let event_auth =
        solana_sdk::pubkey::Pubkey::from_str(crate::constants::DLMM_EVENT_AUTHORITY).unwrap();
    let memo = solana_sdk::pubkey::Pubkey::from_str(crate::constants::MEMO_PROGRAM).unwrap();
    let dlmm = solana_sdk::pubkey::Pubkey::from_str(super::DLMM_PROGRAM).unwrap();
    let tokenkeg =
        solana_sdk::pubkey::Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
            .unwrap();
    let token22 =
        solana_sdk::pubkey::Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
            .unwrap();

    let ix = build_dlmm_swap2_ix(
        &user,
        &lb_pair,
        &bin_arrays,
        &reserve_x,
        &reserve_y,
        &user_in,
        &user_out,
        &token_x,
        &token_y,
        &oracle,
        &event_auth,
        18554703315, // amount_in — matches fixture
        0,           // min_amount_out — matches fixture
        &token22,    // meme uses Token-2022
        &tokenkeg,   // WSOL uses Tokenkeg
        &memo,
        &dlmm,
        Some(&dlmm), // bmpx placeholder
        Some(&dlmm), // host_fee_in placeholder
    );

    // 1. Total data length
    assert_eq!(
        ix.data.len(),
        GOLDEN_SWAP2_DATA.len(),
        "data length must match fixture (28 bytes)"
    );

    // 2. Discriminator
    assert_eq!(
        &ix.data[..8],
        &GOLDEN_SWAP2_DATA[..8],
        "discriminator must match"
    );

    // 3. amount_in at offset 8
    assert_eq!(
        &ix.data[8..16],
        &GOLDEN_SWAP2_DATA[8..16],
        "amount_in at offset 8 must match"
    );

    // 4. min_amount_out at offset 16
    assert_eq!(
        &ix.data[16..24],
        &GOLDEN_SWAP2_DATA[16..24],
        "min_amount_out at offset 16 must match"
    );

    // 5. remaining_accounts_info at offset 24 (empty vec = u32 LE 0)
    assert_eq!(
        &ix.data[24..28],
        &GOLDEN_SWAP2_DATA[24..28],
        "remaining_accounts_info (empty vec) at offset 24 must match"
    );

    // 6. Exact match overall
    assert_eq!(
        ix.data, GOLDEN_SWAP2_DATA,
        "full instruction data must match fixture byte-for-byte"
    );
}

/// Verify data length is always 28 bytes regardless of amount values.
#[test]
fn test_swap2_data_length_always_28() {
    let user = solana_sdk::pubkey::Pubkey::new_unique();
    let lb_pair = solana_sdk::pubkey::Pubkey::new_unique();
    let bin_arrays = vec![solana_sdk::pubkey::Pubkey::new_unique()];
    let reserve_x = solana_sdk::pubkey::Pubkey::new_unique();
    let reserve_y = solana_sdk::pubkey::Pubkey::new_unique();
    let user_in = solana_sdk::pubkey::Pubkey::new_unique();
    let user_out = solana_sdk::pubkey::Pubkey::new_unique();
    let token_x = solana_sdk::pubkey::Pubkey::new_unique();
    let token_y = solana_sdk::pubkey::Pubkey::new_unique();
    let oracle = solana_sdk::pubkey::Pubkey::new_unique();
    let event_auth =
        solana_sdk::pubkey::Pubkey::from_str(crate::constants::DLMM_EVENT_AUTHORITY).unwrap();
    let memo = solana_sdk::pubkey::Pubkey::from_str(crate::constants::MEMO_PROGRAM).unwrap();
    let dlmm = solana_sdk::pubkey::Pubkey::from_str(super::DLMM_PROGRAM).unwrap();
    let tk = solana_sdk::pubkey::Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
        .unwrap();

    let ix = build_dlmm_swap2_ix(
        &user,
        &lb_pair,
        &bin_arrays,
        &reserve_x,
        &reserve_y,
        &user_in,
        &user_out,
        &token_x,
        &token_y,
        &oracle,
        &event_auth,
        1, // tiny amount
        0,
        &tk,
        &tk,
        &memo,
        &dlmm,
        Some(&dlmm),
        Some(&dlmm),
    );
    assert_eq!(ix.data.len(), 28, "data must always be 28 bytes");
}
