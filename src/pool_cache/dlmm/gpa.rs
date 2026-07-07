//! GPAv2-based lb_pair discovery (replaces token-account + PDA scan)

use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use base64::Engine as _;

use super::{parse_optional_pubkey, HTTP_CLIENT};
use crate::constants::DLMM_PROGRAM;

/// Query DLMM lb_pairs containing the given token ordering via `getProgramAccountsV2`.
///
/// Helius credits: 1 credit per call (vs 10 for standard getProgramAccounts).
/// Memcmp filters at offsets 88 (token_x_mint) and 120 (token_y_mint).
/// No dataSize filter — supports both v1 (872B) and v2 (904B) lb_pair accounts.
/// `limit: 1000` is the Helius-recommended minimum for V2; defaults to tiny pages without it.
pub(crate) async fn discover_lb_pairs_via_gpa(
    token_at_88: &str,
    token_at_120: &str,
) -> Vec<(Pubkey, i32, u16, u16, Option<Pubkey>)> {
    let rpc_url = std::env::var("SOLANA_RPC_URL").unwrap_or_default();
    if rpc_url.is_empty() {
        return vec![];
    }

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [DLMM_PROGRAM, {
            "encoding": "base64",
            "filters": [
                {"memcmp": {"offset": 88, "bytes": token_at_88}},
                {"memcmp": {"offset": 120, "bytes": token_at_120}},
            ]
        }]
    });

    let resp = match HTTP_CLIENT.post(&rpc_url).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[DLMM GPA] HTTP error: {e}");
            return vec![];
        }
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(e) => {
            log::warn!("[DLMM GPA] JSON parse error: {e}");
            return vec![];
        }
    };

    // V2 returns {accounts: [...], paginationKey: ...}
    // Standard GPA returns [...]
    let results = json["result"]["accounts"]
        .as_array()
        .or_else(|| json["result"].as_array());
    let results = match results {
        Some(a) => a,
        None => return vec![],
    };

    let mut pools = Vec::with_capacity(results.len());
    for item in results {
        let pubkey_str = match item["pubkey"].as_str() {
            Some(s) => s,
            None => continue,
        };
        let pk = match Pubkey::from_str(pubkey_str) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let data_b64 = match item["account"]["data"][0].as_str() {
            Some(s) => s,
            None => continue,
        };
        let data = match base64::engine::general_purpose::STANDARD.decode(data_b64) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if data.len() < 152 {
            continue;
        }
        let active_id = i32::from_le_bytes(data[76..80].try_into().unwrap_or([0; 4]));
        let bin_step = u16::from_le_bytes(data[80..82].try_into().unwrap_or([0; 2]));
        let base_factor = u16::from_le_bytes(data[84..86].try_into().unwrap_or([0; 2]));
        let bitmap_ext = parse_optional_pubkey(&data, 248);
        pools.push((pk, active_id, bin_step, base_factor, bitmap_ext));
    }
    pools
}
