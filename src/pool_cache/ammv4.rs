//! AMMv4 pool cache fetching
//!
//! Scan AMMv4 pool state accounts from transaction account_keys, read vault balances.

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::{
    cache_key, guess_decimals, is_stablecoin, parse_token_amount, AmmV4PoolInfo, AMMV4_CACHE,
    AMMV4_POOL_CACHE,
};
use crate::constants::{AMM_V4_PROGRAM, NATIVE_SOL_MINT};

/// AMMv4: find pool state account from transaction account_keys, read vault balances
#[allow(dead_code)]
pub async fn fetch_ammv4_now(
    rpc: &RpcClient,
    accounts_json: &str,
    mint_a: &str,
    mint_b: &str,
) -> Option<f64> {
    let mint_key = cache_key(mint_a, mint_b);
    {
        let cache = AMMV4_CACHE.read().unwrap_or_else(|e| {
            log::warn!("AMMv4 reserve cache poisoned, recovering");
            e.into_inner()
        });
        if let Some(entry) = cache.get(&mint_key) {
            if !entry.is_stale() {
                return entry.data;
            }
        }
    }

    let accts: Vec<String> = serde_json::from_str(accounts_json).ok()?;
    if !accts.iter().any(|a| a == AMM_V4_PROGRAM) {
        return None;
    }
    let (liq, pool_addr, pool_data) = discover_and_fetch_ammv4(rpc, &accts, mint_a, mint_b).await?;
    if liq > 0.0 {
        if let Some(pool_info) = parse_ammv4_full_state(&pool_data, mint_a, mint_b, &pool_addr) {
            log::debug!(
                "[AMMv4 POOL INFO] cached pool={} coin={} pc={} market={}",
                &pool_addr[..pool_addr.len().min(12)],
                &pool_info.coin_mint[..pool_info.coin_mint.len().min(12)],
                &pool_info.pc_mint[..pool_info.pc_mint.len().min(12)],
                &pool_info.market[..pool_info.market.len().min(12)],
            );
            let mut cache = AMMV4_POOL_CACHE.write().unwrap_or_else(|e| {
                log::warn!("AMMv4 pool cache poisoned, recovering");
                e.into_inner()
            });
            cache.insert(mint_key.clone(), super::CacheEntry::new(Some(pool_info)));
        } else {
            log::debug!(
                "[AMMv4 POOL INFO] failed to parse pool state for {}/{}",
                mint_a,
                mint_b
            );
        }
        let mut cache = AMMV4_CACHE.write().unwrap_or_else(|e| {
            log::warn!("AMMv4 reserve cache poisoned, recovering");
            e.into_inner()
        });
        cache.insert(mint_key, super::CacheEntry::new(Some(liq)));
    }
    Some(liq)
}

// ============================================================
// Internal implementation
// ============================================================

#[allow(dead_code)]
async fn discover_and_fetch_ammv4(
    rpc: &RpcClient,
    accounts: &[String],
    mint_a: &str,
    mint_b: &str,
) -> Option<(f64, String, Vec<u8>)> {
    let pks: Vec<Pubkey> = accounts
        .iter()
        .filter_map(|s| Pubkey::from_str(s).ok())
        .collect();
    let resp = match rpc.get_multiple_accounts(&pks).await {
        Ok(r) => r,
        Err(e) => {
            log::debug!("[AMMv4] get_multiple_accounts failed: {e}");
            return None;
        }
    };

    let amm_v4_pk = Pubkey::from_str(AMM_V4_PROGRAM).ok()?;

    for (i, acct_opt) in resp.iter().enumerate() {
        let acct = acct_opt.as_ref()?;
        if acct.owner != amm_v4_pk {
            continue;
        }
        if acct.data.len() < 700 || acct.data.len() > 800 {
            continue;
        }

        let pool_addr = pks[i].to_string();

        let mint_a_bytes = Pubkey::from_str(mint_a).ok()?.to_bytes();
        let mint_b_bytes = Pubkey::from_str(mint_b).ok()?.to_bytes();
        let (va, vb) = find_vaults_in_pool_data(&acct.data, &mint_a_bytes, &mint_b_bytes)
            .or_else(|| find_vaults_in_pool_data(&acct.data, &mint_b_bytes, &mint_a_bytes))?;

        log::info!(
            "[AMMv4 LIQUIDITY] pool={} vault_a={} vault_b={}",
            &pool_addr[..pool_addr.len().min(16)],
            &va[..va.len().min(16)],
            &vb[..vb.len().min(16)],
        );

        let liq = fetch_ammv4_vaults(rpc, &va, &vb, mint_a, mint_b).await?;
        let pool_data = acct.data.clone();
        log::info!("[AMMv4 LIQUIDITY] liq_sol={:.2}", liq);
        return Some((liq, pool_addr, pool_data));
    }

    log::debug!("[AMMv4] no pool state found among {} accounts", pks.len());
    None
}

/// Extract full pool info from AMMv4 pool state bytes
#[allow(dead_code)]
fn parse_ammv4_full_state(
    data: &[u8],
    mint_a: &str,
    mint_b: &str,
    pool_address: &str,
) -> Option<AmmV4PoolInfo> {
    let mint_a_bytes = Pubkey::from_str(mint_a).ok()?.to_bytes();
    let mint_b_bytes = Pubkey::from_str(mint_b).ok()?.to_bytes();

    parse_ammv4_with_mints(data, &mint_a_bytes, &mint_b_bytes, pool_address)
        .or_else(|| parse_ammv4_with_mints(data, &mint_b_bytes, &mint_a_bytes, pool_address))
}

#[allow(dead_code)]
fn parse_ammv4_with_mints(
    data: &[u8],
    coin_mint_bytes: &[u8; 32],
    pc_mint_bytes: &[u8; 32],
    pool_address: &str,
) -> Option<AmmV4PoolInfo> {
    let pos = data
        .windows(32)
        .position(|w| w == coin_mint_bytes.as_slice())?;

    if pos + 64 > data.len() || data[pos + 32..pos + 64] != *pc_mint_bytes {
        return None;
    }

    let read_pk = |off: usize| -> Option<String> {
        let bytes: [u8; 32] = data[off..off + 32].try_into().ok()?;
        Some(Pubkey::new_from_array(bytes).to_string())
    };

    Some(AmmV4PoolInfo {
        pool_address: pool_address.to_string(),
        coin_mint: read_pk(pos)?,
        pc_mint: read_pk(pos + 32)?,
        coin_vault: read_pk(pos - 64)?,
        pc_vault: read_pk(pos - 32)?,
        open_orders: read_pk(pos + 96)?,
        target_orders: read_pk(pos + 128)?,
        market: read_pk(pos + 160)?,
        market_program: read_pk(pos + 192)?,
    })
}

/// Locate vault addresses in AMMv4 pool state bytes
#[allow(dead_code)]
fn find_vaults_in_pool_data(
    data: &[u8],
    mint_a: &[u8; 32],
    mint_b: &[u8; 32],
) -> Option<(String, String)> {
    let pos_a = data.windows(32).position(|w| w == mint_a.as_slice())?;

    // mint_a may be coin_mint: token_coin at -64, token_pc at -32
    if pos_a >= 64 {
        let vcoin_bytes: [u8; 32] = data[pos_a - 64..pos_a - 32].try_into().ok()?;
        let vpc_bytes: [u8; 32] = data[pos_a - 32..pos_a].try_into().ok()?;
        let vcoin = Pubkey::new_from_array(vcoin_bytes).to_string();
        let vpc = Pubkey::new_from_array(vpc_bytes).to_string();
        let mint_b_at_pos = pos_a + 32 < data.len() - 32 && data[pos_a + 32..pos_a + 64] == *mint_b;
        if mint_b_at_pos {
            return Some((vcoin, vpc));
        }
    }

    // mint_a may be pc_mint: token_pc at -64, token_coin at -32
    if pos_a >= 32 && pos_a + 32 <= data.len() - 32 {
        let prev_32: &[u8] = &data[pos_a - 32..pos_a];
        if prev_32 == mint_b.as_slice() && pos_a >= 96 {
            let vcoin_bytes: [u8; 32] = data[pos_a - 96..pos_a - 64].try_into().ok()?;
            let vpc_bytes: [u8; 32] = data[pos_a - 64..pos_a - 32].try_into().ok()?;
            let vcoin = Pubkey::new_from_array(vcoin_bytes).to_string();
            let vpc = Pubkey::new_from_array(vpc_bytes).to_string();
            return Some((vcoin, vpc));
        }
    }

    None
}

/// Read balances of two vault token accounts, compute SOL-equivalent TVL
#[allow(dead_code)]
async fn fetch_ammv4_vaults(
    rpc: &RpcClient,
    vault_a: &str,
    vault_b: &str,
    mint_a: &str,
    mint_b: &str,
) -> Option<f64> {
    let va = Pubkey::from_str(vault_a).ok()?;
    let vb = Pubkey::from_str(vault_b).ok()?;

    let accounts = rpc.get_multiple_accounts(&[va, vb]).await.ok()?;

    let raw_a = parse_token_amount(accounts.first()?.as_ref()?.data.as_slice())?;
    let raw_b = parse_token_amount(accounts.get(1)?.as_ref()?.data.as_slice())?;

    let dec_a = guess_decimals(mint_a);
    let dec_b = guess_decimals(mint_b);
    let ui_a = raw_a as f64 / 10f64.powi(dec_a as i32);
    let ui_b = raw_b as f64 / 10f64.powi(dec_b as i32);

    if mint_a == NATIVE_SOL_MINT {
        return Some(ui_a * 2.0);
    }
    if mint_b == NATIVE_SOL_MINT {
        return Some(ui_b * 2.0);
    }

    let sol_price = crate::price::sol_price();
    if sol_price > 0.0 {
        if is_stablecoin(mint_a) {
            return Some(ui_a * 2.0 / sol_price);
        }
        if is_stablecoin(mint_b) {
            return Some(ui_b * 2.0 / sol_price);
        }
    }

    None
}
