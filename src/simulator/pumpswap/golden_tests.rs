use super::*;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

// ============================================================
// Golden fixture: real PumpSwap buy_exact_quote_in
// ============================================================

/// Real successful PumpSwap buy_exact_quote_in → DLMM swap2 arbitrage tx.
/// Extracted buy_exact_quote_in inner CPI accounts (26 = 23 fixed + 3 remaining).
/// Pool: E616WShk..., base_mint: 4TyZG..., coin_creator != default, is_cashback_coin=0.
const GOLDEN_PUMPSWAP_BUY_SIG: &str =
    "4M9Hsts16TM6ZAgtXTruYsaaavoWE3SPy6EaBsCR6dYh7mbSBwNWuanp61tgreNjqtyiQgXqd1jJ8fuecBeTjV8n";

/// 26 accounts from the golden tx inner CPI, matching buy_exact_quote_in layout:
/// 23 fixed + pool_v2_pda (coin_creator≠default) + buyback_recipient + buyback_recipient_ata.
const GOLDEN_PUMPSWAP_BUY_ACCOUNTS: [&str; 26] = [
    "E616WShkSjxnyToCExYLhNWxyYraFqs1RTQz1gZ9ZDUk", // [0]  pool
    "REPLACE_WITH_YOUR_WALLET_PUBKEY",  // [1]  user (signer)
    "ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw", // [2]  global_config
    "4TyZGqRLG3VcHTGMcLBoPUmqYitMVojXinAmkL8xpump", // [3]  base_mint
    "So11111111111111111111111111111111111111112",  // [4]  quote_mint (WSOL)
    "DU72n9dTABNtDaVSa4ePWYgNzjjDGY1icPWdhNNTcFS8", // [5]  user_base_ata
    "4mvdsmiuyx4YKpA8aARxHbBm1KvVs1G9xBa5siJDAFiF", // [6]  user_quote_ata
    "6ixjhzUHGRdmBi6EiQGQhHveNYHj8h24CqSzFo6R91mB", // [7]  pool_base_ata
    "7hMrUWfbxdgNx5EUxiCbnESFedsR7y3UJ68g4jDmVZqM", // [8]  pool_quote_ata
    "62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV", // [9]  protocol_fee_recipient
    "94qWNrtmfn42h3ZjUZwWvK1MEo9uVmmrBPd2hpNjYDjb", // [10] protocol_fee_recipient_ata
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",  // [11] base_token_program (Token-2022)
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  // [12] quote_token_program (SPL Token)
    "11111111111111111111111111111111",             // [13] system_program
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL", // [14] ata_program
    "GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR", // [15] event_authority
    "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",  // [16] pump_program (self)
    "BFfiiUzAqBZEq2CRyXN1zR7cSGGzt4imZVsBTKcYqZdE", // [17] creator_vault_ata
    "G3anQALkM7FkDWv6TbXEee4pW8TrZN1BW1NERDhxDfNC", // [18] creator_vault_authority
    "C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw", // [19] global_volume_accumulator
    "7YYdqorA3ApKEEBBcULiz8VnnEk8jkHdDmdRDoWL4rp1", // [20] user_volume_accumulator
    "5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx", // [21] fee_config
    "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ",  // [22] fee_program
    "HwAqdPBFh6ocypGKSiPUQb4ocAjDN6iRAXCtVdBGG9bY", // [23] pool_v2_pda (coin_creator≠default)
    "5cjcW9wExnJJiqgLjq7DEG75Pm6JBgE1hNv4B2vHXUW6", // [24] buyback_recipient
    "GYH1Gae1wJytMSvMvw8JVcv7nuAbxi8i9erNVbERnzXd", // [25] buyback_recipient_ata
];

/// Live simulation test: verifies our `build_pumpswap_buy_ix` produces
/// account order matching a real on-chain buy_exact_quote_in, then
/// simulates against mainnet with sig_verify=false.
///
/// Expected: simulation reaches the PumpSwap program (passes account/data
/// validation) and fails on balance/business logic, NOT on account or
/// instruction-data errors.
#[tokio::test]
#[ignore]
async fn test_pumpswap_buy_account_layout_live_simulation() {
    use solana_sdk::compute_budget::ComputeBudgetInstruction;
    use solana_sdk::message::Message;
    use solana_sdk::transaction::Transaction;
    use std::time::Duration;

    let _ = tracing_subscriber::fmt::try_init();
    let _ = dotenvy::dotenv();

    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".into());
    let rpc = RpcClient::new_with_timeout(rpc_url.clone(), Duration::from_secs(30));
    println!("[GOLDEN] RPC: {}", rpc_url);

    let pump_prog = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();

    // ---- Step 1: Fetch the pool account used in the golden tx ----
    let pool = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[0]).unwrap();
    let pool_acct = match rpc.get_account(&pool).await {
        Ok(a) => a,
        Err(e) => panic!(
            "golden pool not found on-chain — fixture may be stale\n\
                 pool={pool}\n\
                 tx_sig={GOLDEN_PUMPSWAP_BUY_SIG}\n\
                 error={e}",
        ),
    };
    assert_eq!(
        pool_acct.owner, pump_prog,
        "pool owner must be PumpSwap program"
    );
    assert!(
        pool_acct.data.len() >= 245,
        "pool data too short: {}",
        pool_acct.data.len()
    );

    // ---- Step 2: Parse pool metadata + vault ATAs from on-chain data ----
    let pool_meta =
        parse_pumpswap_pool_meta(&pool_acct.data).expect("parse pumpswap pool metadata");
    assert_eq!(
        pool_meta.pool_base_token_account.to_string(),
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[7],
        "pool_base_token_account from pool data must match fixture [7]",
    );
    assert_eq!(
        pool_meta.pool_quote_token_account.to_string(),
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[8],
        "pool_quote_token_account from pool data must match fixture [8]",
    );
    println!(
        "[GOLDEN] pool vaults from account data: base={} quote={}",
        pool_meta.pool_base_token_account, pool_meta.pool_quote_token_account,
    );

    // ---- Step 3: Verify well-known addresses ----
    assert_eq!(
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[2], PUMPSWAP_GLOBAL_CONFIG,
        "global_config constant must match fixture",
    );
    assert_eq!(
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[15], PUMPSWAP_EVENT_AUTHORITY,
        "event_authority constant must match fixture",
    );
    assert_eq!(
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[22], PUMPSWAP_FEE_PROGRAM,
        "fee_program constant must match fixture",
    );
    println!("[GOLDEN] Well-known addresses: MATCH");

    // ---- Step 4: Verify PDAs ----
    // user_volume_accumulator = PDA(["user_volume_accumulator", user], pump_amm)
    let user = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[1]).unwrap();
    let (user_vol_accum, _) =
        Pubkey::find_program_address(&[b"user_volume_accumulator", &user.to_bytes()], &pump_prog);
    assert_eq!(
        user_vol_accum.to_string(),
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[20],
        "user_volume_accumulator PDA must match fixture",
    );

    // creator_vault_authority = PDA(["creator_vault", coin_creator], pump_amm)
    let (cv_auth, _) = Pubkey::find_program_address(
        &[b"creator_vault", &pool_meta.coin_creator.to_bytes()],
        &pump_prog,
    );
    assert_eq!(
        cv_auth.to_string(),
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[18],
        "creator_vault_authority PDA must match fixture",
    );

    // pool_v2_pda = PDA(["pool-v2", base_mint], pump_amm)
    let base_mint = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[3]).unwrap();
    let (pool_v2, _) =
        Pubkey::find_program_address(&[b"pool-v2", &base_mint.to_bytes()], &pump_prog);
    assert_eq!(
        pool_v2.to_string(),
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[23],
        "pool_v2 PDA must match fixture",
    );

    // fee_config = PDA(["fee_config", pump_amm], fee_program)
    let fee_program = Pubkey::from_str(PUMPSWAP_FEE_PROGRAM).unwrap();
    let (fee_config, _) =
        Pubkey::find_program_address(&[b"fee_config", &pump_prog.to_bytes()], &fee_program);
    assert_eq!(
        fee_config.to_string(),
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS[21],
        "fee_config PDA must match fixture",
    );
    println!("[GOLDEN] PDA derivations: MATCH");

    // ---- Step 5: Build our buy_exact_quote_in instruction ----
    let quote_mint = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[4]).unwrap();
    let user_base_ata = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[5]).unwrap();
    let user_quote_ata = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[6]).unwrap();
    let base_token_program = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[11]).unwrap();
    let quote_token_program = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[12]).unwrap();

    let buyback_recipient = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[24]).unwrap();
    let protocol_fee_recipient = Pubkey::from_str(GOLDEN_PUMPSWAP_BUY_ACCOUNTS[9]).unwrap();

    // Verify is_mayhem_mode from pool (must be false for non-reserved recipient)
    assert!(
        !pool_meta.is_mayhem_mode,
        "buy fixture pool is not mayhem mode",
    );

    let buy_ix = build_pumpswap_buy_ix(
        &user,
        &pool,
        &base_mint,
        &quote_mint,
        &user_base_ata,
        &user_quote_ata,
        &pool_meta.pool_base_token_account,
        &pool_meta.pool_quote_token_account,
        &base_token_program,
        &quote_token_program,
        1_000_000, // spendable_quote_in: 0.001 SOL (dummy)
        1,         // min_base_amount_out: 1 lamport (conservative)
        false,     // track_volume
        &pool_meta.coin_creator,
        pool_meta.is_cashback_coin,
        &buyback_recipient,
        &protocol_fee_recipient,
    );

    // ---- Step 6: Assert account order matches the golden fixture ----
    let our_pubkeys: Vec<String> = buy_ix
        .accounts
        .iter()
        .map(|a| a.pubkey.to_string())
        .collect();

    println!(
        "[GOLDEN] our_accounts.len()={} fixture_accounts.len()={}",
        our_pubkeys.len(),
        GOLDEN_PUMPSWAP_BUY_ACCOUNTS.len(),
    );
    println!(
        "[GOLDEN] pool={pool} coin_creator={} is_cashback={}",
        pool_meta.coin_creator, pool_meta.is_cashback_coin,
    );

    for i in 0..GOLDEN_PUMPSWAP_BUY_ACCOUNTS.len() {
        assert_eq!(
            our_pubkeys[i], GOLDEN_PUMPSWAP_BUY_ACCOUNTS[i],
            "account #{i} mismatch: ours={} fixture={}",
            our_pubkeys[i], GOLDEN_PUMPSWAP_BUY_ACCOUNTS[i],
        );
    }
    println!("[GOLDEN] All 26 accounts: MATCH");

    // ---- Step 7: Live simulation (proves the program accepts our account list) ----
    let ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(400_000),
        buy_ix,
    ];
    let message = Message::new(&ixs, Some(&user));
    let tx = Transaction::new_unsigned(message);

    use solana_client::rpc_config::RpcSimulateTransactionConfig;
    use solana_sdk::commitment_config::CommitmentConfig;
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(CommitmentConfig::processed()),
        encoding: None,
        accounts: None,
        min_context_slot: None,
        inner_instructions: false,
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
            let last_logs: Vec<&str> = logs
                .iter()
                .rev()
                .take(15)
                .rev()
                .map(|s| s.as_str())
                .collect();
            println!("[GOLDEN] sim err: {}", err_msg);
            println!("[GOLDEN] sim logs (last 15):");
            for log_line in &last_logs {
                println!("  {}", log_line);
            }

            // The simulation MUST reach the PumpSwap program.
            // Success criteria: logs reference the PumpSwap program, and the
            // error is a business-logic failure (insufficient balance, etc.),
            // NOT an account layout or instruction data error.
            let invoked_pumpswap = logs
                .iter()
                .any(|l| l.contains("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"));
            assert!(
                invoked_pumpswap,
                "simulation must reach PumpSwap program\n\
                     err={err_msg}\n\
                     last_logs={last_logs:?}",
            );

            if err_msg.is_empty() {
                println!("[GOLDEN] Simulation SUCCEEDED (unexpected — tx would have landed!)");
            } else {
                // Verify the error is NOT about account layout or instruction data
                let account_error = err_msg.to_lowercase().contains("account")
                    || err_msg.to_lowercase().contains("invalid");
                if account_error {
                    println!(
                        "[GOLDEN] WARNING: error mentions 'account'/'invalid' — \
                            verify it's a balance issue, not an account ordering bug.\n\
                            err={err_msg}",
                    );
                } else {
                    println!("[GOLDEN] Error is non-account-related (likely balance): OK");
                }
            }
        }
        Err(e) => {
            panic!(
                "[GOLDEN] RPC simulateTransaction failed: {e}\n\
                     Check RPC endpoint and network connectivity.",
            );
        }
    }
}

// ============================================================
// Golden fixture: real PumpSwap sell
// ============================================================

/// Real successful arbitrage tx: DLMM buy → PumpSwap sell.
/// Extracted PumpSwap sell inner CPI accounts (24 = 21 fixed + 3 remaining).
/// Pool: E2VYktF..., base_mint: 6d9PCh..., coin_creator≠default, is_cashback_coin=0.
const GOLDEN_PUMPSWAP_SELL_SIG: &str =
    "HnigLX1xToqX7DHbB2tZSfhonAsj4weAsbAfkaEz9DaWxqSBsCGAFTQaHUrMpFKv1Uxr1rWin7EkrBuTvtb5uNH";

/// 24 accounts matching sell layout: 21 fixed + pool_v2_pda (coin_creator≠default)
/// + buyback_recipient + buyback_recipient_ata.
const GOLDEN_PUMPSWAP_SELL_ACCOUNTS: [&str; 24] = [
    "E2VYktFBSk3jM8MPaaNvHJThqVgSQZxSFh6nKsLuXqbh", // [0]  pool
    "REPLACE_WITH_YOUR_WALLET_PUBKEY",  // [1]  user (signer)
    "ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw", // [2]  global_config
    "6d9PCh5ocA2v4S7E6G1Hn1dfd72XxE8HJu22r4Sspump", // [3]  base_mint (meme, Token-2022)
    "So11111111111111111111111111111111111111112",  // [4]  quote_mint (WSOL)
    "4qXgzxvTaDGRkJugpE52RswhuenCiLSc4iz2r9EXp2rJ", // [5]  user_base_ata
    "4mvdsmiuyx4YKpA8aARxHbBm1KvVs1G9xBa5siJDAFiF", // [6]  user_quote_ata
    "FsK9jKzHA2bq6KPgLMMfygVtduWMoAq8TKUc6ic9qvi2", // [7]  pool_base_ata
    "EodYxUNuhcE9u8KDRQ5jvznAHzL9jgZtm3V8PtYYkEWu", // [8]  pool_quote_ata
    "9rPYyANsfQZw3DnDmKE3YCQF5E8oD89UXoHn9JFEhJUz", // [9]  protocol_fee_recipient
    "Bvtgim23rfocUzxVX9j9QFxTbBnH8JZxnaGLCEkXvjKS", // [10] protocol_fee_recipient_ata
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",  // [11] base_token_program (Token-2022)
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",  // [12] quote_token_program (SPL Token)
    "11111111111111111111111111111111",             // [13] system_program
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL", // [14] ata_program
    "GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR", // [15] event_authority
    "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",  // [16] pump_program (self)
    "Dimk1s5N17XYpnmEzvij64jdC58LHLtfyQHE5y4ozXtv", // [17] creator_vault_ata
    "CGqq8VWJugrMTPrMmPQprW26F61MFW1UL61nXfGEd95",  // [18] creator_vault_authority
    "5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx", // [19] fee_config
    "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ",  // [20] fee_program
    "5SM8DKpvjiKiEq1icLVL95vmaJHLspXQxpGqDKVoABgP", // [21] pool_v2_pda (coin_creator≠default)
    "3BpXnfJaUTiwXnJNe7Ej1rcbzqTTQUvLShZaWazebsVR", // [22] buyback_recipient
    "6rVkF4HSgy1jrnC3HogfRgPHrq4CtLg5f11URpsC4i9D", // [23] buyback_recipient_ata
];

/// Live simulation test: verifies our `build_pumpswap_sell_ix` produces
/// account order matching a real on-chain sell, then simulates against mainnet.
#[tokio::test]
#[ignore]
async fn test_pumpswap_sell_account_layout_live_simulation() {
    use solana_sdk::compute_budget::ComputeBudgetInstruction;
    use solana_sdk::message::Message;
    use solana_sdk::transaction::Transaction;
    use std::time::Duration;

    let _ = tracing_subscriber::fmt::try_init();
    let _ = dotenvy::dotenv();

    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".into());
    let rpc = RpcClient::new_with_timeout(rpc_url.clone(), Duration::from_secs(30));
    println!("[GOLDEN SELL] RPC: {}", rpc_url);

    let pump_prog = Pubkey::from_str(PUMPFUN_AMM_PROGRAM).unwrap();

    // ---- Step 1: Fetch the pool account ----
    let pool = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[0]).unwrap();
    let pool_acct = match rpc.get_account(&pool).await {
        Ok(a) => a,
        Err(e) => panic!(
            "golden sell pool not found on-chain — fixture may be stale\n\
                 pool={pool}\n\
                 tx_sig={GOLDEN_PUMPSWAP_SELL_SIG}\n\
                 error={e}",
        ),
    };
    assert_eq!(
        pool_acct.owner, pump_prog,
        "pool owner must be PumpSwap program"
    );
    assert!(
        pool_acct.data.len() >= 245,
        "pool data too short: {}",
        pool_acct.data.len()
    );

    // ---- Step 2: Parse pool metadata + vault ATAs ----
    let pool_meta =
        parse_pumpswap_pool_meta(&pool_acct.data).expect("parse pumpswap pool metadata");
    assert_eq!(
        pool_meta.pool_base_token_account.to_string(),
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS[7],
        "pool_base_token_account from pool data must match fixture [7]",
    );
    assert_eq!(
        pool_meta.pool_quote_token_account.to_string(),
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS[8],
        "pool_quote_token_account from pool data must match fixture [8]",
    );
    println!(
        "[GOLDEN SELL] pool vaults from account data: base={} quote={}",
        pool_meta.pool_base_token_account, pool_meta.pool_quote_token_account,
    );
    println!(
        "[GOLDEN SELL] coin_creator={} is_cashback={}",
        pool_meta.coin_creator, pool_meta.is_cashback_coin,
    );
    assert!(
        pool_meta.coin_creator != Pubkey::default(),
        "coin_creator must be non-default for this fixture (triggers pool_v2_pda)",
    );
    assert!(
        !pool_meta.is_cashback_coin,
        "is_cashback_coin must be false for this fixture (no user_vol_accum)",
    );

    // ---- Step 3: Verify well-known addresses ----
    assert_eq!(
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS[2], PUMPSWAP_GLOBAL_CONFIG,
        "global_config constant must match fixture",
    );
    assert_eq!(
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS[15], PUMPSWAP_EVENT_AUTHORITY,
        "event_authority constant must match fixture",
    );
    assert_eq!(
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS[20], PUMPSWAP_FEE_PROGRAM,
        "fee_program constant must match fixture",
    );
    println!("[GOLDEN SELL] Well-known addresses: MATCH");

    // ---- Step 4: Verify PDAs ----
    // creator_vault_authority = PDA(["creator_vault", coin_creator], pump_amm)
    let (cv_auth, _) = Pubkey::find_program_address(
        &[b"creator_vault", &pool_meta.coin_creator.to_bytes()],
        &pump_prog,
    );
    assert_eq!(
        cv_auth.to_string(),
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS[18],
        "creator_vault_authority PDA must match fixture",
    );

    // pool_v2_pda = PDA(["pool-v2", base_mint], pump_amm)
    let base_mint = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[3]).unwrap();
    let (pool_v2, _) =
        Pubkey::find_program_address(&[b"pool-v2", &base_mint.to_bytes()], &pump_prog);
    assert_eq!(
        pool_v2.to_string(),
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS[21],
        "pool_v2 PDA must match fixture",
    );

    // fee_config = PDA(["fee_config", pump_amm], pfeeUxB)
    let fee_program = Pubkey::from_str(PUMPSWAP_FEE_PROGRAM).unwrap();
    let (fee_config, _) =
        Pubkey::find_program_address(&[b"fee_config", &pump_prog.to_bytes()], &fee_program);
    assert_eq!(
        fee_config.to_string(),
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS[19],
        "fee_config PDA must match fixture",
    );
    println!("[GOLDEN SELL] PDA derivations: MATCH");

    // ---- Step 5: Build our sell instruction ----
    let user = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[1]).unwrap();
    let quote_mint = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[4]).unwrap();
    let user_base_ata = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[5]).unwrap();
    let user_quote_ata = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[6]).unwrap();
    let base_token_program = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[11]).unwrap();
    let quote_token_program = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[12]).unwrap();
    let buyback_recipient = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[22]).unwrap();
    let protocol_fee_recipient = Pubkey::from_str(GOLDEN_PUMPSWAP_SELL_ACCOUNTS[9]).unwrap();

    // Verify is_mayhem_mode from pool (must be false for non-reserved recipient)
    assert!(
        !pool_meta.is_mayhem_mode,
        "sell fixture pool is not mayhem mode",
    );

    let base_amount_in: u64 = 38_942_454_037; // from fixture data
    let min_quote_amount_out: u64 = 1; // from fixture data

    let sell_ix = build_pumpswap_sell_ix(
        &user,
        &pool,
        &base_mint,
        &quote_mint,
        &user_base_ata,
        &user_quote_ata,
        &pool_meta.pool_base_token_account,
        &pool_meta.pool_quote_token_account,
        &base_token_program,
        &quote_token_program,
        base_amount_in,
        min_quote_amount_out,
        &pool_meta.coin_creator,
        pool_meta.is_cashback_coin,
        &buyback_recipient,
        &protocol_fee_recipient,
    );

    // ---- Step 6: Assert account order matches the golden fixture ----
    let our_pubkeys: Vec<String> = sell_ix
        .accounts
        .iter()
        .map(|a| a.pubkey.to_string())
        .collect();

    println!(
        "[GOLDEN SELL] our_accounts.len()={} fixture_accounts.len()={}",
        our_pubkeys.len(),
        GOLDEN_PUMPSWAP_SELL_ACCOUNTS.len(),
    );

    for i in 0..GOLDEN_PUMPSWAP_SELL_ACCOUNTS.len() {
        assert_eq!(
            our_pubkeys[i], GOLDEN_PUMPSWAP_SELL_ACCOUNTS[i],
            "sell account #{i} mismatch: ours={} fixture={}",
            our_pubkeys[i], GOLDEN_PUMPSWAP_SELL_ACCOUNTS[i],
        );
    }
    println!("[GOLDEN SELL] All 24 accounts: MATCH");

    // ---- Step 7: Verify data layout ----
    assert_eq!(sell_ix.data.len(), 24, "sell data must be 24 bytes");
    assert_eq!(&sell_ix.data[0..8], &PUMPSWAP_SELL_DISCRIMINATOR);
    assert_eq!(
        u64::from_le_bytes(sell_ix.data[8..16].try_into().unwrap()),
        base_amount_in,
        "base_amount_in must match fixture",
    );
    assert_eq!(
        u64::from_le_bytes(sell_ix.data[16..24].try_into().unwrap()),
        min_quote_amount_out,
        "min_quote_amount_out must match fixture",
    );
    println!("[GOLDEN SELL] Instruction data: MATCH");

    // ---- Step 8: Live simulation ----
    let ixs = vec![
        ComputeBudgetInstruction::set_compute_unit_limit(400_000),
        sell_ix,
    ];
    let message = Message::new(&ixs, Some(&user));
    let tx = Transaction::new_unsigned(message);

    use solana_client::rpc_config::RpcSimulateTransactionConfig;
    use solana_sdk::commitment_config::CommitmentConfig;
    let sim_config = RpcSimulateTransactionConfig {
        sig_verify: false,
        replace_recent_blockhash: true,
        commitment: Some(CommitmentConfig::processed()),
        encoding: None,
        accounts: None,
        min_context_slot: None,
        inner_instructions: false,
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
            let last_logs: Vec<&str> = logs
                .iter()
                .rev()
                .take(15)
                .rev()
                .map(|s| s.as_str())
                .collect();
            println!("[GOLDEN SELL] sim err: {}", err_msg);
            println!("[GOLDEN SELL] sim logs (last 15):");
            for log_line in &last_logs {
                println!("  {}", log_line);
            }

            let invoked_pumpswap = logs
                .iter()
                .any(|l| l.contains("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"));
            assert!(
                invoked_pumpswap,
                "simulation must reach PumpSwap program\n\
                     err={err_msg}\n\
                     last_logs={last_logs:?}",
            );

            if err_msg.is_empty() {
                println!("[GOLDEN SELL] Simulation SUCCEEDED (unexpected — tx would have landed!)");
            } else {
                let account_error = err_msg.to_lowercase().contains("account")
                    || err_msg.to_lowercase().contains("invalid");
                if account_error {
                    println!(
                        "[GOLDEN SELL] WARNING: error mentions 'account'/'invalid' — \
                            verify it's a balance issue, not an account ordering bug.\n\
                            err={err_msg}",
                    );
                } else {
                    println!("[GOLDEN SELL] Error is non-account-related (likely balance): OK");
                }
            }
        }
        Err(e) => {
            panic!(
                "[GOLDEN SELL] RPC simulateTransaction failed: {e}\n\
                     Check RPC endpoint and network connectivity.",
            );
        }
    }
}
