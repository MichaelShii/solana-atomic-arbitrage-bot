//! Helius Sender — ultra-low-latency TX submission with dual routing.

use anyhow::Context;
use base64::Engine;
use solana_sdk::{
    instruction::Instruction,
    message::{v0, VersionedMessage},
    pubkey::Pubkey,
    signature::Keypair,
    transaction::VersionedTransaction,
};
use std::str::FromStr;

/// Helius designated tip accounts (fetched from Sender validation errors).
#[allow(dead_code)]
const TIP_ACCOUNTS: &[&str] = &[
    "4ACfpUFoaSD9bfPdeu6DBt89gB6ENTeHBXCAi87NhDEE",
    "D2L6yPZ2FmmmTKPgzaMKdhu6EWZcTpLy1Vhx8uvZe7NZ",
    "9bnz4RShgq1hAnLnZbP8kbgBg1kEmcJBYQq3gQbmnSta",
    "5VY91ws6B2hMmBFRsXkoAAdsPHBJwRfBht4DXox3xkwn",
    "2nyhqdwKcJZR2vcqCyrYsaPVdAnFoJjiksCXJ7hfEYgD",
    "2q5pghRs6arqVjRvT5gfgWfWcHWmw1ZuCzphgd5KfWGJ",
    "wyvPkWjVZz1M8fHQnMMCDTQDbkManefNNhweYk5WkcF",
    "3KCKozbAaF75qEU33jtzozcJ29yJuaLJTy2jFdzUY8bT",
    "4vieeGHPYPG2MmyPRcYjdiDmmhN3ww7hsFNap8pVN3Ey",
    "4TQLFNWK8AovT1gFvda5jfw2oJeRMKEmw7aH6MGBJ3or",
    "D1Mc6j9xQWgR1o1Z7yU5nVVXFQiAYx7FG9AW1aVfwrUM",
];

#[allow(dead_code)]
const MIN_TIP_LAMPORTS_SWQOS: u64 = 5_000;
#[allow(dead_code)]
const MIN_TIP_LAMPORTS_DUAL: u64 = 200_000;

/// Decompile v0 compiled instructions → high-level `Instruction` objects.
///
/// `all_keys` combines static account_keys with ALT-resolved addresses so
/// every account index resolves correctly.
#[allow(dead_code)]
fn decompile_instructions(v0_msg: &v0::Message, all_keys: &[Pubkey]) -> Vec<Instruction> {
    v0_msg
        .instructions
        .iter()
        .map(|ci| {
            let program_id = all_keys[ci.program_id_index as usize];
            let accounts: Vec<_> = ci
                .accounts
                .iter()
                .map(|&idx| solana_sdk::instruction::AccountMeta {
                    pubkey: all_keys[idx as usize],
                    is_signer: false,
                    is_writable: true,
                })
                .collect();

            Instruction {
                program_id,
                accounts,
                data: ci.data.clone(),
            }
        })
        .collect()
}

#[allow(dead_code)]
async fn add_tip_and_resign(
    wallet: &Keypair,
    tx_bytes: &[u8],
    min_slot: u64,
    swqos_only: bool,
) -> anyhow::Result<Vec<u8>> {
    let mut tx: VersionedTransaction =
        bincode::deserialize(tx_bytes).context("deserialize tx for sender")?;

    let tip_lamports = if swqos_only {
        MIN_TIP_LAMPORTS_SWQOS
    } else {
        MIN_TIP_LAMPORTS_DUAL
    };

    let tip_idx = (min_slot as usize) % TIP_ACCOUNTS.len();
    let tip_account = Pubkey::from_str(TIP_ACCOUNTS[tip_idx])
        .map_err(|e| anyhow::anyhow!("invalid tip account: {e}"))?;

    // Modify v0 message in-place: add tip account + instruction without
    // shifting existing account indices (push at end, zero readonly count).
    let mut v0_msg = match &mut tx.message {
        VersionedMessage::V0(m) => m.clone(),
        _ => anyhow::bail!("sender only supports v0 messages"),
    };

    // Ensure system program is in account_keys.
    let sys_prog = solana_sdk::system_program::ID;
    let sys_prog_idx = v0_msg
        .account_keys
        .iter()
        .position(|k| k == &sys_prog)
        .unwrap_or_else(|| {
            v0_msg.account_keys.push(sys_prog);
            v0_msg.account_keys.len() - 1
        }) as u8;

    // Push tip account at end (no shift of existing indices) and mark
    // all non-signers as writable so the tip account is writable.
    let tip_acct_idx = v0_msg
        .account_keys
        .iter()
        .position(|k| k == &tip_account)
        .unwrap_or_else(|| {
            v0_msg.account_keys.push(tip_account);
            v0_msg.account_keys.len() - 1
        }) as u8;
    v0_msg.header.num_readonly_unsigned_accounts = 0;

    // Build tip instruction by cloning an existing CompiledInstruction.
    let mut tip_data = vec![0u8; 12];
    tip_data[0..4].copy_from_slice(&2u32.to_le_bytes());
    tip_data[4..12].copy_from_slice(&tip_lamports.to_le_bytes());

    let mut tip_ix = v0_msg
        .instructions
        .first()
        .ok_or_else(|| anyhow::anyhow!("tx has no instructions to clone"))?
        .clone();
    tip_ix.program_id_index = sys_prog_idx;
    tip_ix.accounts = vec![0u8, tip_acct_idx]; // payer=0, tip_account
    tip_ix.data = tip_data;
    v0_msg.instructions.push(tip_ix);

    log::debug!(
        "[SENDER DEBUG] keys={} ixns={} sys_prog_idx={} tip_acct_idx={} tip_lamports={}",
        v0_msg.account_keys.len(),
        v0_msg.instructions.len(),
        sys_prog_idx,
        tip_acct_idx,
        tip_lamports,
    );

    let new_tx =
        VersionedTransaction::try_new(VersionedMessage::V0(v0_msg), &[wallet])
            .context("re-sign tx with tip")?;

    Ok(bincode::serialize(&new_tx).context("serialize tx for sender")?)
}

/// Resolve address lookup table contents.
/// Public version that takes an RPC client (used at build time).
pub async fn get_alt_accounts_sync(
    lookups: &[v0::MessageAddressTableLookup],
    rpc: &solana_client::nonblocking::rpc_client::RpcClient,
) -> Vec<solana_sdk::message::AddressLookupTableAccount> {
    use std::sync::LazyLock;
    use std::collections::HashMap;
    use std::sync::Mutex;

    static ALT_CACHE: LazyLock<Mutex<HashMap<Pubkey, Vec<Pubkey>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let mut results = Vec::with_capacity(lookups.len());
    for l in lookups {
        let addresses = {
            let cache = ALT_CACHE.lock().unwrap();
            cache.get(&l.account_key).cloned()
        };
        let addresses = match addresses {
            Some(a) => a,
            None => {
                let fetched = fetch_alt_via_rpc(&l.account_key, rpc).await;
                ALT_CACHE.lock().unwrap().insert(l.account_key, fetched.clone());
                fetched
            }
        };
        results.push(solana_sdk::message::AddressLookupTableAccount {
            key: l.account_key,
            addresses,
        });
    }
    results
}

async fn fetch_alt_via_rpc(
    alt_address: &Pubkey,
    rpc: &solana_client::nonblocking::rpc_client::RpcClient,
) -> Vec<Pubkey> {
    const HEADER_LEN: usize = 32;
    match rpc.get_account_data(alt_address).await {
        Ok(data) if data.len() >= HEADER_LEN => {
            data[HEADER_LEN..]
                .chunks_exact(32)
                .map(|c| Pubkey::try_from(c).unwrap_or_default())
                .collect()
        }
        Err(e) => {
            log::warn!("[SENDER] failed to fetch ALT {alt_address}: {e}");
            vec![]
        }
        _ => vec![],
    }
}

/// Used internally by the submit-time path (uses env var RPC).
#[allow(dead_code)]
async fn get_alt_accounts(
    lookups: &[v0::MessageAddressTableLookup],
) -> Vec<solana_sdk::message::AddressLookupTableAccount> {
    use std::sync::LazyLock;
    use std::collections::HashMap;
    use std::sync::Mutex;

    static ALT_CACHE: LazyLock<Mutex<HashMap<Pubkey, Vec<Pubkey>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let mut results = Vec::with_capacity(lookups.len());
    for l in lookups {
        let addresses = {
            let cache = ALT_CACHE.lock().unwrap();
            cache.get(&l.account_key).cloned()
        };
        let addresses = match addresses {
            Some(a) => a,
            None => {
                let fetched = fetch_alt_account(&l.account_key).await;
                ALT_CACHE.lock().unwrap().insert(l.account_key, fetched.clone());
                fetched
            }
        };
        results.push(solana_sdk::message::AddressLookupTableAccount {
            key: l.account_key,
            addresses,
        });
    }
    results
}

#[allow(dead_code)]
async fn fetch_alt_account(alt_address: &Pubkey) -> Vec<Pubkey> {
    // ALT account layout: 32-byte header (LookupTableMeta) + addresses.
    // LookupTableMeta: deactivation_slot(u64) + last_extended_slot(u64)
    //                  + last_extended_slot_start_index(u8) + __reserved([u8;15])
    const HEADER_LEN: usize = 32;

    use solana_client::nonblocking::rpc_client::RpcClient;
    use std::time::Duration;

    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
    let rpc = RpcClient::new_with_timeout(rpc_url, Duration::from_secs(5));

    match rpc.get_account_data(alt_address).await {
        Ok(data) if data.len() >= HEADER_LEN => {
            let addr_data = &data[HEADER_LEN..];
            addr_data
                .chunks_exact(32)
                .map(|chunk| Pubkey::try_from(chunk).unwrap_or(Pubkey::default()))
                .collect()
        }
        Err(e) => {
            log::warn!("[SENDER] failed to fetch ALT {alt_address}: {e}");
            vec![]
        }
        _ => vec![],
    }
}

#[allow(dead_code)]
pub async fn submit_via_sender(
    wallet: &Keypair,
    tx_bytes: &[u8],
    min_slot: u64,
    sender_endpoint: &str,
    swqos_only: bool,
) -> anyhow::Result<String> {
    let tipped_bytes = add_tip_and_resign(wallet, tx_bytes, min_slot, swqos_only).await?;

    // Add API key for authentication. Sender is free but some plans require it.
    let api_key = std::env::var("HELIUS_API_KEY")
        .unwrap_or_else(|_| "YOUR_HELIUS_API_KEY".to_string());
    let endpoint = if swqos_only {
        format!("{sender_endpoint}?swqos_only=true&api-key={api_key}")
    } else {
        format!("{sender_endpoint}?api-key={api_key}")
    };

    let base64_tx = base64::engine::general_purpose::STANDARD.encode(&tipped_bytes);

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendTransaction",
        "params": [
            base64_tx,
            {
                "encoding": "base64",
                "skipPreflight": true,
                "maxRetries": 0
            }
        ]
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build reqwest client")?;

    let resp = client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .context("sender HTTP POST")?;

    let status = resp.status();
    let text = resp.text().await.context("read sender response")?;

    if !status.is_success() {
        anyhow::bail!("sender returned {status}: {text}");
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).context("parse sender response")?;

    if let Some(err) = json.get("error") {
        anyhow::bail!("sender RPC error: {err}");
    }

    json.get("result")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
        .context("sender response missing 'result' field")
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        compute_budget::ComputeBudgetInstruction,
        instruction::Instruction,
        native_token::LAMPORTS_PER_SOL,
        signature::Keypair,
        signer::Signer,
        transaction::Transaction,
    };
    use std::str::FromStr;

    /// Test Sender with a trivial SOL transfer that includes a tip.
    /// This isolates our TX format from the complex arbitrage builders.
    /// WARNING: sends real SOL if BOT_PRIVATE_KEY is set. Run manually:
    ///   cargo test --release sender::test_sender -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_sender_with_simple_transfer() {
        // Load the real wallet (requires BOT_PRIVATE_KEY env var).
        let pk_b58 = std::env::var("BOT_PRIVATE_KEY").expect("BOT_PRIVATE_KEY not set");
        let wallet = Keypair::from_base58_string(&pk_b58);

        // Build a v0 transaction with: compute_budget + tip transfer + self-transfer.
        let tip_account =
            Pubkey::from_str("4ACfpUFoaSD9bfPdeu6DBt89gB6ENTeHBXCAi87NhDEE").unwrap();
        let payer = wallet.pubkey();

        let mut instructions: Vec<Instruction> = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(100_000),
            ComputeBudgetInstruction::set_compute_unit_price(50_000),
            system_instruction::transfer(&payer, &tip_account, 200_000), // dual-routing tip
            system_instruction::transfer(&payer, &payer, 1_000), // self-transfer
        ];

        // Legacy transaction format.
        let blockhash = solana_sdk::hash::Hash::new_unique();
        let message = solana_sdk::message::Message::new(&instructions, Some(&payer));
        let mut tx = Transaction::new_unsigned(message);
        tx.sign(&[&wallet], blockhash);
        let tx_bytes = bincode::serialize(&tx).unwrap();

        let endpoint = "http://your-sender-endpoint.example.com";
        let result = submit_via_sender_raw(&tx_bytes, &endpoint, false).await;
        println!("=== Legacy TX, tip=200k, dual routing ===");
        match &result {
            Ok(sig) => println!("✅ SENDER OK: sig={sig}"),
            Err(e) => println!("❌ SENDER FAIL: {e}"),
        }

        // Test 2: SWQOS-only with 5000 tip (legacy).
        let insns_swqos: Vec<Instruction> = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(100_000),
            ComputeBudgetInstruction::set_compute_unit_price(50_000),
            system_instruction::transfer(&payer, &tip_account, 5_000),
            system_instruction::transfer(&payer, &payer, 1_000),
        ];
        let msg2 = solana_sdk::message::Message::new(&insns_swqos, Some(&payer));
        let mut tx2 = Transaction::new_unsigned(msg2);
        tx2.sign(&[&wallet], blockhash);
        let tx2_bytes = bincode::serialize(&tx2).unwrap();

        let result2 = submit_via_sender_raw(&tx2_bytes, &endpoint, true).await;
        println!("=== Legacy TX, tip=5k, swqos_only ===");
        match &result2 {
            Ok(sig) => println!("✅ SENDER OK: sig={sig}"),
            Err(e) => println!("❌ SENDER FAIL: {e}"),
        }

        // Test 3: Use add_tip_and_resign to inject tip into a v0 TX (our actual use case).
        // Need to build a proper v0 TX first.
        use solana_sdk::signer::Signer;
        let insns_no_tip: Vec<Instruction> = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(100_000),
            ComputeBudgetInstruction::set_compute_unit_price(50_000),
            system_instruction::transfer(&payer, &payer, 1_000),
        ];
        let v0_msg = solana_sdk::message::v0::Message::try_compile(
            &payer,
            &insns_no_tip,
            &[],
            blockhash,
        )
        .unwrap();
        let mut v0_tx = VersionedTransaction::try_new(
            solana_sdk::message::VersionedMessage::V0(v0_msg),
            &[&wallet],
        )
        .unwrap();
        let v0_bytes = bincode::serialize(&v0_tx).unwrap();

        let tipped = add_tip_and_resign(&wallet, &v0_bytes, 42, true).await;
        match tipped {
            Ok(tipped_bytes) => {
                let result3 = submit_via_sender_raw(&tipped_bytes, &endpoint, true).await;
                println!("=== V0 TX (no ALT), injected tip, swqos_only ===");
                match &result3 {
                    Ok(sig) => println!("✅ SENDER OK: sig={sig}"),
                    Err(e) => println!("❌ SENDER FAIL: {e}"),
                }
            }
            Err(e) => println!("❌ add_tip_and_resign failed: {e}"),
        }

        // Test 4: V0 TX with ALT (fetched from mainnet).
        test_sender_v0_with_alt(&wallet, &endpoint).await;
    }

    async fn test_sender_v0_with_alt(wallet: &Keypair, endpoint: &str) {
        use solana_client::nonblocking::rpc_client::RpcClient;
        use std::time::Duration;

        let payer = wallet.pubkey();
        let tip_account =
            Pubkey::from_str("4ACfpUFoaSD9bfPdeu6DBt89gB6ENTeHBXCAi87NhDEE").unwrap();

        // Use our onchain arb ALT.
        let alt_key = Pubkey::from_str(
            "A97gxq4Zd4PiJ6XnZ68ZwTLfZDbmeBaC2GSxJmgugpGo",
        )
        .unwrap();

        // Fetch ALT contents from RPC.
        let rpc_url = std::env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());
        let rpc = RpcClient::new_with_timeout(rpc_url, Duration::from_secs(10));
        let alt_data = match rpc.get_account_data(&alt_key).await {
            Ok(d) => d,
            Err(e) => {
                println!("⚠️  Cannot fetch ALT, skipping v0+ALT test: {e}");
                return;
            }
        };

        // Parse ALT: 32-byte header + addresses
        let alt_addresses: Vec<Pubkey> = if alt_data.len() >= 32 {
            alt_data[32..]
                .chunks_exact(32)
                .map(|c| Pubkey::try_from(c).unwrap_or_default())
                .collect()
        } else {
            vec![]
        };
        println!("  ALT has {} addresses", alt_addresses.len());

        let alt_account = solana_sdk::message::AddressLookupTableAccount {
            key: alt_key,
            addresses: alt_addresses,
        };

        // Build instructions that use some ALT accounts.
        let blockhash = solana_sdk::hash::Hash::new_unique();
        let instructions: Vec<Instruction> = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(200_000),
            ComputeBudgetInstruction::set_compute_unit_price(50_000),
            // Self-transfer (these accounts are in static keys, not ALT).
            system_instruction::transfer(&payer, &payer, 1_000),
        ];

        let v0_msg = v0::Message::try_compile(&payer, &instructions, &[alt_account], blockhash)
            .expect("compile v0 with ALT");
        let mut v0_tx = VersionedTransaction::try_new(
            VersionedMessage::V0(v0_msg),
            &[wallet],
        )
        .unwrap();
        let v0_bytes = bincode::serialize(&v0_tx).unwrap();

        let (n_keys, n_ixns) = match &v0_tx.message {
            VersionedMessage::V0(m) => (m.account_keys.len(), m.instructions.len()),
            _ => (0, 0),
        };
        println!(
            "  Built v0+ALT TX: {} static keys, {} ixns, {} bytes",
            n_keys, n_ixns, v0_bytes.len(),
        );

        // Inject tip and test.
        let tipped = add_tip_and_resign(wallet, &v0_bytes, 42, true).await;
        match tipped {
            Ok(tipped_bytes) => {
                let result4 = submit_via_sender_raw(&tipped_bytes, endpoint, true).await;
                println!("=== V0 TX (with ALT), injected tip, swqos_only ===");
                match &result4 {
                    Ok(sig) => println!("✅ SENDER OK: sig={sig}"),
                    Err(e) => println!("❌ SENDER FAIL: {e}"),
                }
            }
            Err(e) => println!("❌ add_tip_and_resign failed: {e}"),
        }
    }

    /// Raw sender submit without tip injection (for pre-tipped TXs).
    async fn submit_via_sender_raw(
        tx_bytes: &[u8],
        sender_endpoint: &str,
        swqos_only: bool,
    ) -> anyhow::Result<String> {
        let endpoint = if swqos_only {
            format!("{sender_endpoint}?swqos_only=true")
        } else {
            sender_endpoint.to_string()
        };
        let base64_tx =
            base64::engine::general_purpose::STANDARD.encode(tx_bytes);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                base64_tx,
                {"encoding": "base64", "skipPreflight": true, "maxRetries": 0}
            ]
        });

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        let resp = client
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("sender returned {status}: {text}");
        }
        let json: serde_json::Value = serde_json::from_str(&text)?;
        if let Some(err) = json.get("error") {
            anyhow::bail!("sender RPC error: {err}");
        }
        Ok(json["result"].as_str().unwrap_or("?").to_string())
    }
}
