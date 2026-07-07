//! Bin array parsing + fresh bin fetch + full discovery fetch

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::super::{parse_token_amount, DlmmBin, DlmmPoolMetadata, DlmmPoolReserves};
use super::{parse_optional_pubkey, BINS_PER_ARRAY};
use crate::constants::DLMM_PROGRAM;

/// Parse one bin_array account's raw data, appending parsed DlmmBin entries.
///
/// BinArray layout (Anchor): discriminator(8) + index(i64) + version(u8) + padding(7)
///   + lb_pair(Pubkey=32) = 56 header, then 69 Bin structs (144 bytes each).
///
/// Bin layout (v2, 144 bytes): amount_x(u64) + amount_y(u64) + price(u128)
///   + liquidity_supply(u128) + fulfilled_order_amount_x(u64)
///   + fulfilled_order_amount_y(u64) + limit_order_fee_ask_side(u64)
///   + limit_order_fee_bid_side(u64) + fee_amount_x_per_token_stored(u128)
///   + fee_amount_y_per_token_stored(u128) + open_order_amount(u64)
///   + total_processing_order_amount(u64) + processed_order_remaining_amount(u64)
///   + order_age(u32) + limit_order_ask_side(u8) + padding(3).
///
/// NOTE: There are NO reserve_x/reserve_y fields in the DLMM Bin struct.
/// amount_x and amount_y are the actual available reserves for swap.
/// The previous code was reading garbage bytes (processed_order_remaining_amount
/// and order_age/padding) as fake "reserves", causing wildly inflated profit estimates.
pub(crate) fn parse_bin_array_data(bdata: &[u8], out: &mut Vec<DlmmBin>) {
    const HEADER_SIZE: usize = 8 + 48; // discriminator + BinArray header
    if bdata.len() < HEADER_SIZE {
        return;
    }
    let arr_idx = i64::from_le_bytes(bdata[8..16].try_into().unwrap_or([0; 8]));
    let version = bdata.get(16).copied().unwrap_or(1);
    let stride: usize = match version {
        1 => 128,
        2 => 144,
        _ => {
            log::warn!(
                "[DLMM] unknown bin version={} — skipping bin_array",
                version
            );
            return;
        }
    };
    log::debug!(
        "[DLMM-BIN] arr_idx={} version={} stride={} data_len={}",
        arr_idx,
        version,
        stride,
        bdata.len()
    );
    // Dump first bin raw bytes for debugging
    if bdata.len() > HEADER_SIZE + 32 {
        let bo = HEADER_SIZE;
        let dump: String = bdata[bo..bo + 32]
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        log::debug!("[DLMM-BIN] first_bin_raw[0..32]={}", dump);
    }
    for i in 0..70i32 {
        let offset: usize = HEADER_SIZE + (i as usize) * stride;
        let min_needed = 16; // amount_x(8) + amount_y(8) — both v1 and v2 have these
        if offset + min_needed > bdata.len() {
            break;
        }
        let amount_x = u64::from_le_bytes(bdata[offset..offset + 8].try_into().unwrap_or([0; 8]));
        let amount_y =
            u64::from_le_bytes(bdata[offset + 8..offset + 16].try_into().unwrap_or([0; 8]));
        // DLMM v2 Bin struct does NOT have reserve_x/reserve_y fields.
        // amount_x/amount_y are the actual available reserves for swap.
        let bin_id = arr_idx.saturating_mul(70i64).saturating_add(i as i64) as i32;
        out.push(DlmmBin {
            bin_id,
            amount_x,
            amount_y,
            reserve_x: 0,
            reserve_y: 0,
        });
    }
}

/// Fetch fresh DLMM reserves using permanently cached metadata.
///
/// Only 2 RPCs (vs 7-8 for full discovery):
///   1. getAccount(lb_pair) → active_id
///   2. getMultipleAccounts(reserve_x, reserve_y, bin_array_0, bin_array_1, bin_array_2)
///
/// PDA derivations are client-side computations (no RPC).
pub(crate) async fn fetch_reserves_with_metadata(
    rpc: &RpcClient,
    meta: &DlmmPoolMetadata,
) -> Option<DlmmPoolReserves> {
    let lb_pair_pk = Pubkey::from_str(&meta.lb_pair).ok()?;
    let dlmm = match Pubkey::from_str(DLMM_PROGRAM) {
        Ok(p) => p,
        Err(_) => return None,
    };

    // Step 1: Read lb_pair account to get current active_id (1 RPC)
    let lb_pair_acct = rpc.get_account(&lb_pair_pk).await.ok()?;
    let data = &lb_pair_acct.data;
    if data.len() < 152 {
        return None;
    }
    let active_id = i32::from_le_bytes(data[76..80].try_into().ok()?);
    let base_factor = u16::from_le_bytes(data[84..86].try_into().ok()?);
    let bitmap_ext = parse_optional_pubkey(data, 248);

    let token_x_pk = Pubkey::from_str(&meta.token_x_mint).ok()?;
    let token_y_pk = Pubkey::from_str(&meta.token_y_mint).ok()?;

    // Derive reserve PDAs (client-side)
    let (reserve_x_pda, _) =
        Pubkey::find_program_address(&[&lb_pair_pk.to_bytes(), &token_x_pk.to_bytes()], &dlmm);
    let (reserve_y_pda, _) =
        Pubkey::find_program_address(&[&lb_pair_pk.to_bytes(), &token_y_pk.to_bytes()], &dlmm);

    // Derive bin_array PDAs around active_id (client-side)
    // Fetch 5 bins (offsets -2..=2) to cover larger swaps that cross multiple bins.
    let active_bin_array_idx = active_id / BINS_PER_ARRAY;
    let mut bin_array_pubkeys: Vec<Pubkey> = Vec::with_capacity(5);
    let mut all_addrs: Vec<Pubkey> = Vec::with_capacity(7);
    all_addrs.push(reserve_x_pda);
    all_addrs.push(reserve_y_pda);
    for offset in -2i32..=2i32 {
        let idx = (active_bin_array_idx + offset) as i64;
        let (pda, _) = Pubkey::find_program_address(
            &[b"bin_array", &lb_pair_pk.to_bytes(), &idx.to_le_bytes()],
            &dlmm,
        );
        bin_array_pubkeys.push(pda);
        all_addrs.push(pda);
    }

    // Step 2: Batch fetch ALL dynamic data in one call (1 RPC)
    let accounts = rpc.get_multiple_accounts(&all_addrs).await.ok()?;

    // Parse pool-level reserves (accounts[0], accounts[1])
    let reserve_x = accounts
        .first()
        .and_then(|a| a.as_ref())
        .and_then(|a| parse_token_amount(&a.data))
        .unwrap_or(0);
    let reserve_y = accounts
        .get(1)
        .and_then(|a| a.as_ref())
        .and_then(|a| parse_token_amount(&a.data))
        .unwrap_or(0);

    // Parse bin arrays (accounts[2..])
    let mut dlmm_bins: Vec<DlmmBin> = Vec::with_capacity(207);
    for i in 0..bin_array_pubkeys.len() {
        if let Some(acct) = &accounts[i + 2] {
            parse_bin_array_data(&acct.data, &mut dlmm_bins);
        }
    }

    let bin_array_addresses: Vec<String> =
        bin_array_pubkeys.iter().map(|p| p.to_string()).collect();

    Some(DlmmPoolReserves {
        lb_pair: meta.lb_pair.clone(),
        token_x_mint: meta.token_x_mint.clone(),
        token_y_mint: meta.token_y_mint.clone(),
        reserve_x,
        reserve_y,
        reserve_x_address: reserve_x_pda.to_string(),
        reserve_y_address: reserve_y_pda.to_string(),
        bin_array_addresses,
        bins: dlmm_bins,
        bin_step: meta.bin_step,
        base_factor,
        active_id,
        bin_array_bitmap_extension: bitmap_ext.map(|p| p.to_string()),
        sqrt_price: 0,
        tick_current_index: 0,
        fee_rate: 0,
    })
}

/// Re-fetch bins for a specific lb_pair fresh from chain (bypasses cache).
/// Used to verify high-profit opportunities haven't been arbed during cache TTL.
pub async fn fetch_bins_fresh(rpc: &RpcClient, lb_pair: &str) -> anyhow::Result<Vec<DlmmBin>> {
    let lb_pair_pk = Pubkey::from_str(lb_pair)?;
    let dlmm = Pubkey::from_str(DLMM_PROGRAM)?;

    let cache = crate::grpc_stream::global_cache();
    let data: Vec<u8> = if let Some((cached, cached_slot)) = cache.get_with_slot(lb_pair) {
        let latest = cache.latest_slot();
        if latest > 0 && latest.saturating_sub(cached_slot) <= 2 {
            cached
        } else {
            rpc.get_account(&lb_pair_pk).await?.data
        }
    } else {
        rpc.get_account(&lb_pair_pk).await?.data
    };
    if data.len() < 152 {
        anyhow::bail!("lb_pair account too short");
    }
    let active_id = i32::from_le_bytes(data[76..80].try_into().unwrap());

    let active_bin_array_idx = active_id / BINS_PER_ARRAY;
    let mut addresses = Vec::with_capacity(3);
    for offset in -1i32..=1i32 {
        let idx = (active_bin_array_idx + offset) as i64;
        let (pda, _) = Pubkey::find_program_address(
            &[b"bin_array", &lb_pair_pk.to_bytes(), &idx.to_le_bytes()],
            &dlmm,
        );
        addresses.push(pda);
    }

    // Try gRPC cache for each bin_array, collect misses for batched RPC
    let mut bins: Vec<DlmmBin> = Vec::with_capacity(207);
    let mut rpc_miss: Vec<Pubkey> = Vec::with_capacity(3);
    let cache = crate::grpc_stream::global_cache();
    let latest = cache.latest_slot();
    for addr in &addresses {
        let addr_str = addr.to_string();
        if let Some((cached, cached_slot)) = cache.get_with_slot(&addr_str) {
            if latest > 0 && latest.saturating_sub(cached_slot) <= 2 {
                parse_bin_array_data(&cached, &mut bins);
                continue;
            }
        }
        rpc_miss.push(*addr);
    }

    if !rpc_miss.is_empty() {
        let accounts = rpc.get_multiple_accounts(&rpc_miss).await?;
        for acct in accounts.iter().flatten() {
            parse_bin_array_data(&acct.data, &mut bins);
        }
    }
    Ok(bins)
}

pub(crate) async fn fetch_reserves_inner(
    rpc: &RpcClient,
    lb_pair_pk: &Pubkey,
    dlmm: &Pubkey,
) -> Option<DlmmPoolReserves> {
    // Step 1: Read lb_pair to parse authoritative token mints, active_id, bin_step
    let lb_pair_str = lb_pair_pk.to_string();
    let cache = crate::grpc_stream::global_cache();
    let data: Vec<u8> = if let Some((cached, cached_slot)) = cache.get_with_slot(&lb_pair_str) {
        let latest = cache.latest_slot();
        if latest > 0 && latest.saturating_sub(cached_slot) <= 2 {
            log::debug!("[GRPC-CACHE] hit lb_pair {}", &lb_pair_str[..lb_pair_str.len().min(12)]);
            cached
        } else {
            let acct = rpc.get_account(lb_pair_pk).await.ok()?;
            acct.data
        }
    } else {
        let acct = rpc.get_account(lb_pair_pk).await.ok()?;
        acct.data
    };
    if data.len() < 152 {
        return None;
    }

    // Offsets verified against official IDL (idl.ts):
    //   LbPair uses bytemuck + repr(C), struct layout:
    //   discrim[8] + StaticParams[32] + VarParams[32]
    //   + bumpSeed[1] + binStepSeed[2] + pairType[1]
    //   + activeId:i32[4] + binStep:u16[2] + status[1]
    //   + requireBaseFactorSeed[1] + baseFactorSeed[2] + activationType[1]
    //   + creatorPoolOnOffControl[1]
    //   + tokenXMint:pubkey[32] + tokenYMint:pubkey[32] + ...
    //   activeId @ 76, binStep @ 80, tokenXMint @ 88, tokenYMint @ 120
    let active_id = i32::from_le_bytes(data[76..80].try_into().ok()?);
    let bin_step = u16::from_le_bytes(data[80..82].try_into().ok()?);
    let base_factor = u16::from_le_bytes(data[84..86].try_into().ok()?);
    let bitmap_ext = parse_optional_pubkey(&data, 248);
    let token_x_mint = {
        let bytes: [u8; 32] = data[88..120].try_into().ok()?;
        Pubkey::new_from_array(bytes).to_string()
    };
    let token_y_mint = {
        let bytes: [u8; 32] = data[120..152].try_into().ok()?;
        Pubkey::new_from_array(bytes).to_string()
    };

    let token_x_pk = Pubkey::from_str(&token_x_mint).ok()?;
    let token_y_pk = Pubkey::from_str(&token_y_mint).ok()?;

    // Step 2: Derive reserve PDAs
    // Seeds verified against official SDK: [lbPair, tokenMint] — NO "liquidity" prefix
    let (reserve_x_pda, _) =
        Pubkey::find_program_address(&[&lb_pair_pk.to_bytes(), &token_x_pk.to_bytes()], dlmm);
    let (reserve_y_pda, _) =
        Pubkey::find_program_address(&[&lb_pair_pk.to_bytes(), &token_y_pk.to_bytes()], dlmm);

    // Step 3: Read reserves (gRPC cache first, fall back to RPC)
    let latest = cache.latest_slot();
    let try_cache = |pda: &Pubkey| -> Option<u64> {
        let key = pda.to_string();
        cache
            .get_with_slot(&key)
            .filter(|(_, s)| latest > 0 && latest.saturating_sub(*s) <= 2)
            .and_then(|(d, _)| parse_token_amount(&d))
    };
    let (reserve_x, reserve_y) = match (try_cache(&reserve_x_pda), try_cache(&reserve_y_pda)) {
        (Some(rx), Some(ry)) => (rx, ry),
        _ => {
            let accounts = rpc
                .get_multiple_accounts(&[reserve_x_pda, reserve_y_pda])
                .await
                .ok()?;
            (
                accounts.first().and_then(|a| a.as_ref()).and_then(|a| parse_token_amount(&a.data)).unwrap_or(0),
                accounts.get(1).and_then(|a| a.as_ref()).and_then(|a| parse_token_amount(&a.data)).unwrap_or(0),
            )
        }
    };

    // Step 4: Derive bin_array PDAs around the active bin
    // Fetch 5 bins (offsets -2..=2) for larger swaps.
    let active_bin_array_idx = active_id / BINS_PER_ARRAY;
    let mut bin_array_addresses = Vec::with_capacity(5);
    for offset in -2i32..=2i32 {
        let idx = (active_bin_array_idx + offset) as i64;
        let (pda, _) = Pubkey::find_program_address(
            &[b"bin_array", &lb_pair_pk.to_bytes(), &idx.to_le_bytes()],
            dlmm,
        );
        bin_array_addresses.push(pda.to_string());
    }

    // Step 5: Fetch bin array account data (gRPC cache first, fall back to RPC)
    let mut dlmm_bins: Vec<DlmmBin> = Vec::with_capacity(207);
    let mut rpc_miss: Vec<Pubkey> = Vec::with_capacity(3);

    for addr_str in &bin_array_addresses {
        if let Some((cached, s)) = cache.get_with_slot(addr_str) {
            if latest > 0 && latest.saturating_sub(s) <= 2 {
                parse_bin_array_data(&cached, &mut dlmm_bins);
                continue;
            }
        }
        if let Ok(pk) = Pubkey::from_str(addr_str) {
            rpc_miss.push(pk);
        }
    }

    for pk in &rpc_miss {
        if let Ok(acct) = rpc.get_account(pk).await {
            parse_bin_array_data(&acct.data, &mut dlmm_bins);
        }
    }

    Some(DlmmPoolReserves {
        lb_pair: lb_pair_pk.to_string(),
        token_x_mint: token_x_mint.clone(),
        token_y_mint: token_y_mint.clone(),
        reserve_x,
        reserve_y,
        reserve_x_address: reserve_x_pda.to_string(),
        reserve_y_address: reserve_y_pda.to_string(),
        bin_array_addresses,
        bins: dlmm_bins,
        bin_step,
        base_factor,
        active_id,
        bin_array_bitmap_extension: bitmap_ext.map(|p| p.to_string()),
        sqrt_price: 0,
        tick_current_index: 0,
        fee_rate: 0,
    })
}
