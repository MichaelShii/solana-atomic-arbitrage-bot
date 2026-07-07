//! Durable nonce support — eliminates blockhash RPC from the critical path.
//!
//! Nonce account data layout (version 1, "current"):
//!   offset  0: version: u32 = 1
//!   offset  4: state: u32 = 1 (initialized)
//!   offset  8: nonce_value: Pubkey (low 8 bytes = nonce)
//!   offset 40: blockhash: Hash (32 bytes) — the durable blockhash
//!   offset 72: lamports_per_signature: u64

use anyhow::Context;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::hash::Hash;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Mutex;

const NONCE_ACCOUNT_SIZE: usize = 80;

/// Cached nonce data — avoid re-reading the nonce account on every TX build.
///
/// The nonce only changes when our TX lands (advanceNonce bumps the value),
/// so we can reuse the cached data across failed/scrapped attempts.
static NONCE_CACHE: Mutex<Option<(Pubkey, NonceData)>> = Mutex::new(None);

pub struct NonceData {
    pub blockhash: Hash,
    pub nonce_value: u64,
}

impl NonceData {
    /// Parse from raw nonce account data bytes.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < NONCE_ACCOUNT_SIZE {
            return None;
        }
        let version = u32::from_le_bytes(data[0..4].try_into().ok()?);
        let state = u32::from_le_bytes(data[4..8].try_into().ok()?);
        if version != 1 || state != 1 {
            return None;
        }
        let blockhash_bytes: [u8; 32] = data[40..72].try_into().ok()?;
        let nonce_value = u64::from_le_bytes(data[8..16].try_into().ok()?);
        Some(Self {
            blockhash: Hash::new_from_array(blockhash_bytes),
            nonce_value,
        })
    }
}

/// Fetch nonce data: cache-first, RPC fallback.
///
/// Cached data is valid until the nonce is advanced (i.e. until our TX lands).
/// On TX failure the nonce stays unchanged, so the cache remains fresh.
pub async fn cached_nonce_data(
    rpc: &RpcClient,
    nonce_account: &Pubkey,
) -> anyhow::Result<NonceData> {
    {
        let cache = NONCE_CACHE.lock().unwrap();
        if let Some((ref pk, ref nd)) = *cache {
            if pk == nonce_account {
                log::debug!("[NONCE CACHE] hit nonce={}", nonce_account);
                return Ok(NonceData {
                    blockhash: nd.blockhash,
                    nonce_value: nd.nonce_value,
                });
            }
        }
    }
    // Cache miss — fetch from RPC
    let nd = fetch_nonce_data(rpc, nonce_account).await?;
    let mut cache = NONCE_CACHE.lock().unwrap();
    *cache = Some((*nonce_account, NonceData {
        blockhash: nd.blockhash,
        nonce_value: nd.nonce_value,
    }));
    Ok(nd)
}

/// Invalidate the nonce cache (called after successful TX submission).
/// The next TX build will re-read the nonce account from RPC.
pub fn invalidate_nonce_cache() {
    let mut cache = NONCE_CACHE.lock().unwrap();
    *cache = None;
    log::debug!("[NONCE CACHE] invalidated after successful submit");
}

/// Fetch nonce account data from RPC (uncached).
pub async fn fetch_nonce_data(rpc: &RpcClient, nonce_account: &Pubkey) -> anyhow::Result<NonceData> {
    let account = rpc
        .get_account(nonce_account)
        .await
        .context("fetch nonce account")?;
    NonceData::parse(&account.data)
        .context("parse nonce account — is this a valid initialized nonce account?")
}

/// Build the `AdvanceNonceAccount` system instruction (instruction index 4).
///
/// Must be the first instruction in the transaction so the nonce
/// is consumed before any swap operations.
pub fn build_advance_nonce_ix(nonce_account: &Pubkey, authority: &Pubkey) -> Instruction {
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let recent_blockhashes =
        Pubkey::from_str("SysvarRecentB1ockHashes11111111111111111111").unwrap();
    Instruction {
        program_id: system_program,
        accounts: vec![
            AccountMeta::new(*nonce_account, false),
            AccountMeta::new_readonly(recent_blockhashes, false),
            AccountMeta::new_readonly(*authority, true),
        ],
        data: vec![4, 0, 0, 0],
    }
}
