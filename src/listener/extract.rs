//! Transaction log extraction — parse pool addresses and mints from getTransaction responses
//!
//! Each DEX has a different inner instruction account layout; handled centrally here for DLMM, CPMM, Whirlpool.

use log::debug;
use serde_json::Value;

/// Fetch a DLMM swap transaction and extract the lb_pair + token mints from account keys.
///
/// Account layout for DLMM Swap2 (from IDL): 0=lb_pair, 5=token_x_mint, 6=token_y_mint
pub async fn extract_lb_pair_from_tx(rpc_url: &str, sig: &str) -> Option<(String, String, String)> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [sig, {"encoding": "json", "maxSupportedTransactionVersion": 0}]
    });

    let resp: Value = client
        .post(rpc_url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let result = resp.get("result")?;

    let account_keys: Vec<&str> = result
        .get("transaction")?
        .get("message")?
        .get("accountKeys")?
        .as_array()?
        .iter()
        .filter_map(|ak| ak.as_str().or_else(|| ak.get("pubkey")?.as_str()))
        .collect();

    let dlmm_prog = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo";

    for inner in result.get("meta")?.get("innerInstructions")?.as_array()? {
        for ix in inner.get("instructions")?.as_array()? {
            let prog_idx = ix.get("programIdIndex")?.as_u64()? as usize;
            if account_keys.get(prog_idx) == Some(&dlmm_prog) {
                let acct_indices: Vec<usize> = ix
                    .get("accounts")?
                    .as_array()?
                    .iter()
                    .filter_map(|a| a.as_u64().map(|n| n as usize))
                    .collect();

                let lb_pair = account_keys.get(*acct_indices.first()?)?.to_string();
                let mint_a = account_keys.get(*acct_indices.get(5)?)?.to_string();
                let mint_b = account_keys.get(*acct_indices.get(6)?)?.to_string();

                debug!(
                    "[LB_PAIR] extracted from tx sig={} lb_pair={}",
                    &sig[..sig.len().min(12)],
                    &lb_pair[..lb_pair.len().min(12)],
                );
                return Some((mint_a, mint_b, lb_pair));
            }
        }
    }

    None
}

/// Fetch a CPMM swap transaction and extract pool address + token mints from account keys.
///
/// CPMM Swap accounts (Raydium CP-Swap): 3=pool_state, 10=input_mint, 11=output_mint.
pub async fn extract_cpmm_pool_from_tx(
    rpc_url: &str,
    sig: &str,
) -> Option<(String, String, String)> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [sig, {"encoding": "json", "maxSupportedTransactionVersion": 0}]
    });

    let resp: Value = client
        .post(rpc_url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let result = resp.get("result")?;

    let account_keys: Vec<&str> = result
        .get("transaction")?
        .get("message")?
        .get("accountKeys")?
        .as_array()?
        .iter()
        .filter_map(|ak| ak.as_str().or_else(|| ak.get("pubkey")?.as_str()))
        .collect();

    let cpmm_prog = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C";

    for inner in result.get("meta")?.get("innerInstructions")?.as_array()? {
        for ix in inner.get("instructions")?.as_array()? {
            let prog_idx = ix.get("programIdIndex")?.as_u64()? as usize;
            if account_keys.get(prog_idx) == Some(&cpmm_prog) {
                let acct_indices: Vec<usize> = ix
                    .get("accounts")?
                    .as_array()?
                    .iter()
                    .filter_map(|a| a.as_u64().map(|n| n as usize))
                    .collect();

                // CPMM Swap: 3=pool_state, 10=input_mint, 11=output_mint
                let pool_addr = account_keys.get(*acct_indices.get(3)?)?.to_string();
                let mint_a = account_keys.get(*acct_indices.get(10)?)?.to_string();
                let mint_b = account_keys.get(*acct_indices.get(11)?)?.to_string();

                debug!(
                    "[CPMM_POOL] extracted pool={} mints={}/{}",
                    &pool_addr[..pool_addr.len().min(12)],
                    &mint_a[..mint_a.len().min(12)],
                    &mint_b[..mint_b.len().min(12)],
                );
                return Some((mint_a, mint_b, pool_addr));
            }
        }
    }

    None
}

/// Fetch a Whirlpool swap transaction, extract pool address from inner instruction,
/// then read pool account data to get token mints.
///
/// Whirlpool swap accounts (Orca swap_v2): 2=whirlpool (pool state).
/// Mints are stored in pool account data (not instruction accounts) at borsh offsets.
pub async fn extract_whirlpool_from_tx(
    rpc_url: &str,
    sig: &str,
) -> Option<(String, String, String)> {
    let client = reqwest::Client::new();

    // Step 1: getTransaction to find pool address
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTransaction",
        "params": [sig, {"encoding": "json", "maxSupportedTransactionVersion": 0}]
    });

    let resp: Value = client
        .post(rpc_url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let result = resp.get("result")?;

    let account_keys: Vec<&str> = result
        .get("transaction")?
        .get("message")?
        .get("accountKeys")?
        .as_array()?
        .iter()
        .filter_map(|ak| ak.as_str().or_else(|| ak.get("pubkey")?.as_str()))
        .collect();

    let whirlpool_prog = "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc";

    // Find pool address from Whirlpool swap inner instruction (accounts[2])
    let mut pool_addr: Option<String> = None;
    for inner in result.get("meta")?.get("innerInstructions")?.as_array()? {
        for ix in inner.get("instructions")?.as_array()? {
            let prog_idx = ix.get("programIdIndex")?.as_u64()? as usize;
            if account_keys.get(prog_idx) == Some(&whirlpool_prog) {
                let acct_indices: Vec<usize> = ix
                    .get("accounts")?
                    .as_array()?
                    .iter()
                    .filter_map(|a| a.as_u64().map(|n| n as usize))
                    .collect();

                pool_addr = Some(account_keys.get(*acct_indices.get(2)?)?.to_string());
                break;
            }
        }
        if pool_addr.is_some() {
            break;
        }
    }

    let pool_addr = pool_addr?;

    // Step 2: getAccountInfo to read pool data and extract mints
    let body2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getAccountInfo",
        "params": [pool_addr, {"encoding": "base64"}]
    });

    let resp2: Value = client
        .post(rpc_url)
        .json(&body2)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    let acct_data = resp2
        .get("result")?
        .get("value")?
        .get("data")?
        .get(0)?
        .as_str()?;

    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(acct_data)
        .ok()?;

    if data.len() < 243 {
        return None;
    }

    let read_pubkey = |off: usize| -> Option<String> {
        let bytes: [u8; 32] = data[off..off + 32].try_into().ok()?;
        Some(solana_sdk::pubkey::Pubkey::new_from_array(bytes).to_string())
    };

    // Borsh offsets: 99=token_mint_a, 179=token_mint_b
    let mint_a = read_pubkey(99)?;
    let mint_b = read_pubkey(179)?;

    debug!(
        "[WHIRLPOOL_POOL] extracted pool={} mints={}/{}",
        &pool_addr[..pool_addr.len().min(12)],
        &mint_a[..mint_a.len().min(12)],
        &mint_b[..mint_b.len().min(12)],
    );
    Some((mint_a, mint_b, pool_addr))
}
