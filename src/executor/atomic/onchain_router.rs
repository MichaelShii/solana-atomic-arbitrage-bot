//! On-chain arbitrage program router — builds the single CPI instruction
//! that replaces the two-leg swap instructions in the legacy builders.
//!
//! Each function returns a `(Instruction, last_valid_block_height)` pair.
//! The WSOL wrap/unwrap, ATA creation, and compute-budget IXs stay in
//! `mod.rs` — this module only builds the arbitrage program IX.

use anyhow::Context;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use crate::config::AppConfig;
use crate::constants;
use crate::pool_cache::DlmmPoolReserves;
use crate::simulator;
use crate::simulator::{
    pumpswap_coin_creator_vault_ata, pumpswap_coin_creator_vault_authority,
    pumpswap_fee_config_pda, pumpswap_user_vol_accumulator,
    pumpswap_user_vol_accumulator_quote_ata,
};

use super::helpers::pick_pumpswap_protocol_fee_recipient;

// ── route_pump_to_dlmm: PumpSwap buy → DLMM sell ─────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_route_pump_to_dlmm(
    wallet_pubkey: &Pubkey,
    user_sol_ata: &Pubkey,
    user_meme_ata: &Pubkey,
    meme_mint: &Pubkey,
    investment_lamports: u64,
    min_meme_out: u64,
    min_profit_lamports: u64,
    dlmm: &DlmmPoolReserves,
    pool_meta: &simulator::PumpSwapPoolMeta,
    pool: &Pubkey,
    meme_token_program: &Pubkey,
    config: &AppConfig,
    last_valid_block_height: u64,
) -> anyhow::Result<(Instruction, u64)> {
    let sol_mint = Pubkey::from_str(constants::NATIVE_SOL_MINT)?;
    let sol_token_program = Pubkey::from_str(constants::TOKEN_PROGRAM)?;
    let arb_program_id = Pubkey::from_str(&config.execution_routing.onchain_program_id)?;

    let dlmm_sol_is_x = dlmm.token_x_mint == constants::NATIVE_SOL_MINT;

    let mut accounts: Vec<AccountMeta> = Vec::new();

    // ── Shared [0..=2] ──────────────────────────────────────────────
    accounts.push(AccountMeta::new(*wallet_pubkey, true)); // 0: user (signer)
    accounts.push(AccountMeta::new(*user_sol_ata, false)); // 1: user_sol_ata
    accounts.push(AccountMeta::new(*user_meme_ata, false)); // 2: user_meme_ata

    // ── PumpSwap Buy [3..=25+remaining] ─────────────────────────────
    let _pump_base = 3;
    push_pumpswap_buy_accounts(
        &mut accounts,
        wallet_pubkey,
        user_sol_ata,
        user_meme_ata,
        meme_mint,
        &sol_mint,
        meme_token_program,
        &sol_token_program,
        pool,
        pool_meta,
    );

    let pump_remaining_count = pumpswap_buy_remaining_count(pool_meta);

    // ── DLMM section ────────────────────────────────────────────────
    let dlmm_program = Pubkey::from_str(constants::DLMM_PROGRAM)?;
    let lb_pair = Pubkey::from_str(&dlmm.lb_pair)?;
    let reserve_x = Pubkey::from_str(&dlmm.reserve_x_address)?;
    let reserve_y = Pubkey::from_str(&dlmm.reserve_y_address)?;
    let (oracle, _) =
        Pubkey::find_program_address(&[b"oracle", &lb_pair.to_bytes()], &dlmm_program);
    let memo_program = Pubkey::from_str(constants::MEMO_PROGRAM)?;
    let event_auth = Pubkey::from_str(constants::DLMM_EVENT_AUTHORITY)?;
    let bitmap = dlmm
        .bin_array_bitmap_extension
        .as_deref()
        .and_then(|s| Pubkey::from_str(s).ok())
        .unwrap_or(dlmm_program);
    let bin_array_count = dlmm.bin_array_addresses.len().min(5);
    let bin_arrays: Vec<Pubkey> = dlmm
        .bin_array_addresses
        .iter()
        .take(bin_array_count)
        .map(|a| Pubkey::from_str(a).unwrap())
        .collect();

    push_dlmm_accounts(
        &mut accounts,
        &dlmm_program,
        &lb_pair,
        &bitmap,
        &reserve_x,
        &reserve_y,
        &oracle,
        &dlmm_program, // host_fee_in = DLMM program (skip fee, avoids Token-2022 TransferFee issue)
        &memo_program,
        &event_auth,
        &bin_arrays,
    );


    // ── Build IX data ───────────────────────────────────────────────
    let ix_data = build_ix_data(
        constants::ROUTE_PUMP_TO_DLMM_DISC,
        investment_lamports,
        min_profit_lamports,
        min_meme_out,
        false, // track_volume
        dlmm_sol_is_x,
        pump_remaining_count,
        bin_array_count as u8,
    );

    Ok((
        Instruction {
            program_id: arb_program_id,
            accounts,
            data: ix_data,
        },
        last_valid_block_height,
    ))
}

// ── route_dlmm_to_pump: DLMM buy → PumpSwap sell ─────────────────────

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_route_dlmm_to_pump(
    wallet_pubkey: &Pubkey,
    user_sol_ata: &Pubkey,
    user_meme_ata: &Pubkey,
    meme_mint: &Pubkey,
    investment_lamports: u64,
    min_meme_out: u64,
    min_profit_lamports: u64,
    dlmm: &DlmmPoolReserves,
    pool_meta: &simulator::PumpSwapPoolMeta,
    pool: &Pubkey,
    meme_token_program: &Pubkey,
    config: &AppConfig,
    last_valid_block_height: u64,
) -> anyhow::Result<(Instruction, u64)> {
    let sol_mint = Pubkey::from_str(constants::NATIVE_SOL_MINT)?;
    let sol_token_program = Pubkey::from_str(constants::TOKEN_PROGRAM)?;
    let arb_program_id = Pubkey::from_str(&config.execution_routing.onchain_program_id)?;

    let dlmm_sol_is_x = dlmm.token_x_mint == constants::NATIVE_SOL_MINT;

    let mut accounts: Vec<AccountMeta> = Vec::new();

    // ── Shared [0..=2] ──────────────────────────────────────────────
    accounts.push(AccountMeta::new(*wallet_pubkey, true)); // 0: user (signer)
    accounts.push(AccountMeta::new(*user_sol_ata, false)); // 1: user_sol_ata
    accounts.push(AccountMeta::new(*user_meme_ata, false)); // 2: user_meme_ata

    // ── DLMM section [3..] ──────────────────────────────────────────
    let dlmm_program = Pubkey::from_str(constants::DLMM_PROGRAM)?;
    let lb_pair = Pubkey::from_str(&dlmm.lb_pair)?;
    let reserve_x = Pubkey::from_str(&dlmm.reserve_x_address)?;
    let reserve_y = Pubkey::from_str(&dlmm.reserve_y_address)?;
    let (oracle, _) =
        Pubkey::find_program_address(&[b"oracle", &lb_pair.to_bytes()], &dlmm_program);
    let memo_program = Pubkey::from_str(constants::MEMO_PROGRAM)?;
    let event_auth = Pubkey::from_str(constants::DLMM_EVENT_AUTHORITY)?;
    let bitmap = dlmm
        .bin_array_bitmap_extension
        .as_deref()
        .and_then(|s| Pubkey::from_str(s).ok())
        .unwrap_or(dlmm_program);
    let bin_array_count = dlmm.bin_array_addresses.len().min(5);
    let bin_arrays: Vec<Pubkey> = dlmm
        .bin_array_addresses
        .iter()
        .take(bin_array_count)
        .map(|a| Pubkey::from_str(a).unwrap())
        .collect();

    push_dlmm_accounts(
        &mut accounts,
        &dlmm_program,
        &lb_pair,
        &bitmap,
        &reserve_x,
        &reserve_y,
        &oracle,
        &dlmm_program, // host_fee_in = DLMM program (skip fee, avoids Token-2022 mismatch)
        &memo_program,
        &event_auth,
        &bin_arrays,
    );

    // ── PumpSwap Sell section ───────────────────────────────────────
    push_pumpswap_sell_accounts(
        &mut accounts,
        wallet_pubkey,
        user_sol_ata,
        user_meme_ata,
        meme_mint,
        &sol_mint,
        meme_token_program,
        &sol_token_program,
        pool,
        pool_meta,
    );


    let pump_remaining_count = pumpswap_sell_remaining_count(pool_meta);

    // ── Build IX data ───────────────────────────────────────────────
    let ix_data = build_ix_data(
        constants::ROUTE_DLMM_TO_PUMP_DISC,
        investment_lamports,
        min_profit_lamports,
        min_meme_out,
        false, // track_volume
        dlmm_sol_is_x,
        pump_remaining_count,
        bin_array_count as u8,
    );

    Ok((
        Instruction {
            program_id: arb_program_id,
            accounts,
            data: ix_data,
        },
        last_valid_block_height,
    ))
}

// ── Shared account builders ───────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn push_dlmm_accounts(
    accounts: &mut Vec<AccountMeta>,
    dlmm_program: &Pubkey,
    lb_pair: &Pubkey,
    bitmap: &Pubkey,
    reserve_x: &Pubkey,
    reserve_y: &Pubkey,
    oracle: &Pubkey,
    host_fee: &Pubkey,
    memo_program: &Pubkey,
    event_auth: &Pubkey,
    bin_arrays: &[Pubkey],
) {
    accounts.push(AccountMeta::new_readonly(*dlmm_program, false)); // 0: program
    accounts.push(AccountMeta::new(*lb_pair, false)); // 1: lb_pair
    accounts.push(AccountMeta::new(*bitmap, false)); // 2: bitmap (writable since DLMM v0.12.0)
    accounts.push(AccountMeta::new(*reserve_x, false)); // 3: reserve_x
    accounts.push(AccountMeta::new(*reserve_y, false)); // 4: reserve_y
    accounts.push(AccountMeta::new(*oracle, false)); // 5: oracle
    accounts.push(AccountMeta::new(*host_fee, false)); // 6: host_fee_in
    accounts.push(AccountMeta::new_readonly(*memo_program, false)); // 7: memo
    accounts.push(AccountMeta::new_readonly(*event_auth, false)); // 8: event_authority
    for bin in bin_arrays {
        accounts.push(AccountMeta::new(*bin, false)); // 9+: bin arrays
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_pumpswap_buy_accounts(
    accounts: &mut Vec<AccountMeta>,
    user: &Pubkey,
    user_sol_ata: &Pubkey,
    user_meme_ata: &Pubkey,
    meme_mint: &Pubkey,
    sol_mint: &Pubkey,
    base_token_program: &Pubkey,
    quote_token_program: &Pubkey,
    pool: &Pubkey,
    pool_meta: &simulator::PumpSwapPoolMeta,
) {
    let pump_program = Pubkey::from_str(constants::PUMPFUN_AMM_PROGRAM).unwrap();
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let ata_program = Pubkey::from_str(constants::ATA_PROGRAM).unwrap();
    let global_config = Pubkey::from_str(constants::PUMPSWAP_GLOBAL_CONFIG).unwrap();
    let event_authority = Pubkey::from_str(constants::PUMPSWAP_EVENT_AUTHORITY).unwrap();
    let global_vol_accum = Pubkey::from_str(constants::PUMPSWAP_GLOBAL_VOLUME_ACCUMULATOR).unwrap();
    let fee_program = Pubkey::from_str(constants::PUMPSWAP_FEE_PROGRAM).unwrap();

    let protocol_fee_recipient = pick_pumpswap_protocol_fee_recipient(pool_meta.is_mayhem_mode);
    let protocol_fee_ata = simulator::ata_addr(&protocol_fee_recipient, sol_mint, quote_token_program);

    let creator_vault_authority = pumpswap_coin_creator_vault_authority(&pool_meta.coin_creator);
    let creator_vault_ata =
        pumpswap_coin_creator_vault_ata(&creator_vault_authority, sol_mint, quote_token_program);

    let fee_config = pumpswap_fee_config_pda();
    let user_vol_accum = pumpswap_user_vol_accumulator(user);

    // Fixed 23: exact order from on-chain program PumpSwap buy_exact_quote_in
    accounts.push(AccountMeta::new(*pool, false)); // 0: pool
    accounts.push(AccountMeta::new(*user, true)); // 1: user
    accounts.push(AccountMeta::new_readonly(global_config, false)); // 2: global_config
    accounts.push(AccountMeta::new_readonly(*meme_mint, false)); // 3: base_mint
    accounts.push(AccountMeta::new_readonly(*sol_mint, false)); // 4: quote_mint
    accounts.push(AccountMeta::new(*user_meme_ata, false)); // 5: user_base_ata
    accounts.push(AccountMeta::new(*user_sol_ata, false)); // 6: user_quote_ata
    accounts.push(AccountMeta::new(pool_meta.pool_base_token_account, false)); // 7: pool_base_ata
    accounts.push(AccountMeta::new(pool_meta.pool_quote_token_account, false)); // 8: pool_quote_ata
    accounts.push(AccountMeta::new_readonly(protocol_fee_recipient, false)); // 9: protocol_fee_recipient
    accounts.push(AccountMeta::new(protocol_fee_ata, false)); // 10: protocol_fee_ata
    accounts.push(AccountMeta::new_readonly(*base_token_program, false)); // 11: base_token_program
    accounts.push(AccountMeta::new_readonly(*quote_token_program, false)); // 12: quote_token_program
    accounts.push(AccountMeta::new_readonly(system_program, false)); // 13: system_program
    accounts.push(AccountMeta::new_readonly(ata_program, false)); // 14: ata_program
    accounts.push(AccountMeta::new_readonly(event_authority, false)); // 15: event_authority
    accounts.push(AccountMeta::new_readonly(pump_program, false)); // 16: pump_program
    accounts.push(AccountMeta::new(creator_vault_ata, false)); // 17: coin_creator_vault_ata
    accounts.push(AccountMeta::new_readonly(creator_vault_authority, false)); // 18: coin_creator_vault_auth
    accounts.push(AccountMeta::new_readonly(global_vol_accum, false)); // 19: global_vol_accum
    accounts.push(AccountMeta::new(user_vol_accum, false)); // 20: user_vol_accum
    accounts.push(AccountMeta::new_readonly(fee_config, false)); // 21: fee_config
    accounts.push(AccountMeta::new_readonly(fee_program, false)); // 22: fee_program

    // Remaining (matches PumpSwap IDL append_swap_remaining_accounts order)
    if pool_meta.is_cashback_coin {
        let cashback_ata = pumpswap_user_vol_accumulator_quote_ata(user, sol_mint, quote_token_program);
        accounts.push(AccountMeta::new(cashback_ata, false));
    }
    if pool_meta.coin_creator != Pubkey::default() {
        accounts.push(AccountMeta::new_readonly(
            simulator::pumpswap_pool_v2_pda(meme_mint),
            false,
        ));
    }
    let buyback_recipient = Pubkey::from_str(constants::PUMPSWAP_BUYBACK_FEE_RECIPIENT).unwrap();
    let buyback_ata = simulator::ata_addr(&buyback_recipient, sol_mint, quote_token_program);
    accounts.push(AccountMeta::new_readonly(buyback_recipient, false));
    accounts.push(AccountMeta::new(buyback_ata, false));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn push_pumpswap_sell_accounts(
    accounts: &mut Vec<AccountMeta>,
    user: &Pubkey,
    user_sol_ata: &Pubkey,
    user_meme_ata: &Pubkey,
    meme_mint: &Pubkey,
    sol_mint: &Pubkey,
    base_token_program: &Pubkey,
    quote_token_program: &Pubkey,
    pool: &Pubkey,
    pool_meta: &simulator::PumpSwapPoolMeta,
) {
    let pump_program = Pubkey::from_str(constants::PUMPFUN_AMM_PROGRAM).unwrap();
    let system_program = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    let ata_program = Pubkey::from_str(constants::ATA_PROGRAM).unwrap();
    let global_config = Pubkey::from_str(constants::PUMPSWAP_GLOBAL_CONFIG).unwrap();
    let event_authority = Pubkey::from_str(constants::PUMPSWAP_EVENT_AUTHORITY).unwrap();
    let fee_program = Pubkey::from_str(constants::PUMPSWAP_FEE_PROGRAM).unwrap();

    let protocol_fee_recipient = pick_pumpswap_protocol_fee_recipient(pool_meta.is_mayhem_mode);
    let protocol_fee_ata = simulator::ata_addr(&protocol_fee_recipient, sol_mint, quote_token_program);

    let creator_vault_authority = pumpswap_coin_creator_vault_authority(&pool_meta.coin_creator);
    let creator_vault_ata =
        pumpswap_coin_creator_vault_ata(&creator_vault_authority, sol_mint, quote_token_program);

    let fee_config = pumpswap_fee_config_pda();

    // Fixed 21: exact order from on-chain program PumpSwap sell
    accounts.push(AccountMeta::new(*pool, false)); // 0: pool
    accounts.push(AccountMeta::new(*user, true)); // 1: user
    accounts.push(AccountMeta::new_readonly(global_config, false)); // 2: global_config
    accounts.push(AccountMeta::new_readonly(*meme_mint, false)); // 3: base_mint
    accounts.push(AccountMeta::new_readonly(*sol_mint, false)); // 4: quote_mint
    accounts.push(AccountMeta::new(*user_meme_ata, false)); // 5: user_base_ata
    accounts.push(AccountMeta::new(*user_sol_ata, false)); // 6: user_quote_ata
    accounts.push(AccountMeta::new(pool_meta.pool_base_token_account, false)); // 7: pool_base_ata
    accounts.push(AccountMeta::new(pool_meta.pool_quote_token_account, false)); // 8: pool_quote_ata
    accounts.push(AccountMeta::new_readonly(protocol_fee_recipient, false)); // 9: protocol_fee_recipient
    accounts.push(AccountMeta::new(protocol_fee_ata, false)); // 10: protocol_fee_ata
    accounts.push(AccountMeta::new_readonly(*base_token_program, false)); // 11: base_token_program
    accounts.push(AccountMeta::new_readonly(*quote_token_program, false)); // 12: quote_token_program
    accounts.push(AccountMeta::new_readonly(system_program, false)); // 13: system_program
    accounts.push(AccountMeta::new_readonly(ata_program, false)); // 14: ata_program
    accounts.push(AccountMeta::new_readonly(event_authority, false)); // 15: event_authority
    accounts.push(AccountMeta::new_readonly(pump_program, false)); // 16: pump_program
    accounts.push(AccountMeta::new(creator_vault_ata, false)); // 17: coin_creator_vault_ata
    accounts.push(AccountMeta::new_readonly(creator_vault_authority, false)); // 18: coin_creator_vault_auth
    accounts.push(AccountMeta::new_readonly(fee_config, false)); // 19: fee_config
    accounts.push(AccountMeta::new_readonly(fee_program, false)); // 20: fee_program

    // Remaining: Sell adds user_vol_accumulator (writable) when cashback
    if pool_meta.is_cashback_coin {
        let cashback_ata = pumpswap_user_vol_accumulator_quote_ata(user, sol_mint, quote_token_program);
        accounts.push(AccountMeta::new(cashback_ata, false));
        accounts.push(AccountMeta::new(pumpswap_user_vol_accumulator(user), false));
    }
    if pool_meta.coin_creator != Pubkey::default() {
        accounts.push(AccountMeta::new_readonly(
            simulator::pumpswap_pool_v2_pda(meme_mint),
            false,
        ));
    }
    let buyback_recipient = Pubkey::from_str(constants::PUMPSWAP_BUYBACK_FEE_RECIPIENT).unwrap();
    let buyback_ata = simulator::ata_addr(&buyback_recipient, sol_mint, quote_token_program);
    accounts.push(AccountMeta::new_readonly(buyback_recipient, false));
    accounts.push(AccountMeta::new(buyback_ata, false));
}

// ── Helpers ───────────────────────────────────────────────────────────

pub(crate) fn pumpswap_buy_remaining_count(pool_meta: &simulator::PumpSwapPoolMeta) -> u8 {
    let mut n: u8 = 2; // buyback_recipient + buyback_recipient_ata (always)
    if pool_meta.is_cashback_coin {
        n += 1; // cashback_ata
    }
    if pool_meta.coin_creator != Pubkey::default() {
        n += 1; // pool_v2_pda
    }
    n
}

fn pumpswap_sell_remaining_count(pool_meta: &simulator::PumpSwapPoolMeta) -> u8 {
    let mut n: u8 = 2; // buyback_recipient + buyback_recipient_ata (always)
    if pool_meta.is_cashback_coin {
        n += 2; // cashback_ata + user_vol_accum
    }
    if pool_meta.coin_creator != Pubkey::default() {
        n += 1; // pool_v2_pda
    }
    n
}

#[allow(clippy::too_many_arguments)]
fn build_ix_data(
    disc: [u8; 8],
    amount_in: u64,
    min_profit: u64,
    min_meme_out: u64,
    track_volume: bool,
    dlmm_sol_is_x: bool,
    pump_remaining_count: u8,
    dlmm_bin_array_count: u8,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&disc);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_profit.to_le_bytes());
    data.extend_from_slice(&min_meme_out.to_le_bytes());
    data.push(track_volume as u8);
    data.push(dlmm_sol_is_x as u8);
    data.push(pump_remaining_count);
    data.push(dlmm_bin_array_count);
    data
}

// ── Full TX builders (pricing + WSOL wrap/unwrap + onchain IX) ──────

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::address_lookup_table::state::AddressLookupTable;
use solana_sdk::address_lookup_table::AddressLookupTableAccount;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use tokio::sync::RwLock;


/// DB-backed cache for reserve→token_program mappings. Survives restarts.
static DB_TP_CACHE: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| {
    let map = crate::persistence::reserve_owners_load_all();
    log::info!("[TP CACHE] loaded {} reserve→owner mappings from DB", map.len());
    Mutex::new(map)
});

/// Pre-warm the TP cache by batch-reading all known DLMM pool reserve accounts.
/// Called once at startup after gRPC cache has initial data.
pub(crate) async fn warmup_tp_cache(rpc: &RpcClient) {
    
    let metadata = crate::pool_cache::all_dlmm_metadata();
    if metadata.is_empty() {
        return;
    }

    let dlmm = match Pubkey::from_str(crate::constants::DLMM_PROGRAM) {
        Ok(p) => p,
        Err(_) => return,
    };

    // Collect all unique reserve addresses not already in DB
    let cached: Vec<String> = DB_TP_CACHE.lock().unwrap().keys().cloned().collect();
    let mut to_fetch: Vec<Pubkey> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for meta in &metadata {
        if let (Ok(lb), Ok(tx), Ok(ty)) = (
            Pubkey::from_str(&meta.lb_pair),
            Pubkey::from_str(&meta.token_x_mint),
            Pubkey::from_str(&meta.token_y_mint),
        ) {
            for token in [tx, ty] {
                let (pda, _) = Pubkey::find_program_address(
                    &[&lb.to_bytes(), &token.to_bytes()],
                    &dlmm,
                );
                let key = pda.to_string();
                if !cached.contains(&key) && seen.insert(key.clone()) {
                    to_fetch.push(pda);
                }
            }
        }
    }

    if to_fetch.is_empty() {
        return;
    }

    log::info!(
        "[TP WARMUP] fetching {} reserve accounts from RPC...",
        to_fetch.len()
    );

    // Batch fetch in chunks of 100
    for chunk in to_fetch.chunks(100) {
        if let Ok(accounts) = rpc.get_multiple_accounts(chunk).await {
            for (pk, acct_opt) in chunk.iter().zip(accounts.iter()) {
                if let Some(acct) = acct_opt {
                    let owner = acct.owner.to_string();
                    if owner == crate::constants::TOKEN_PROGRAM
                        || owner == crate::constants::TOKEN22_PROGRAM
                    {
                        let key = pk.to_string();
                        crate::persistence::reserve_owner_save(&key, &owner);
                        DB_TP_CACHE.lock().unwrap().insert(key, owner);
                    }
                }
            }
        }
    }

    let count = DB_TP_CACHE.lock().unwrap().len();
    log::info!("[TP WARMUP] done, {} total reserve→owner mappings", count);
}



/// ALT cache with slot-based TTL to detect on-chain updates.
static ALT_CACHE: LazyLock<RwLock<Option<(Pubkey, AddressLookupTableAccount, u64)>>> =
    LazyLock::new(|| RwLock::new(None));

/// Refetch ALT if cached > ALT_CACHE_SLOTS ago (~3 min at 400ms/slot).
const ALT_CACHE_SLOTS: u64 = 500;

pub(crate) async fn get_alt(
    rpc: &RpcClient,
    alt_address: Pubkey,
) -> anyhow::Result<AddressLookupTableAccount> {
    // Check cache with slot TTL
    if let Some((addr, alt, cached_slot)) = ALT_CACHE.read().await.as_ref() {
        if *addr == alt_address {
            let current_slot = rpc.get_slot().await.unwrap_or(0);
            if current_slot.saturating_sub(*cached_slot) < ALT_CACHE_SLOTS {
                return Ok(alt.clone());
            }
        }
    }
    let account = rpc
        .get_account(&alt_address)
        .await
        .context("fetch ALT account")?;
    let alt_state = AddressLookupTable::deserialize(&account.data).context("deserialize ALT state")?;
    let alt = AddressLookupTableAccount {
        key: alt_address,
        addresses: alt_state.addresses.to_vec(),
    };
    let current_slot = rpc.get_slot().await.unwrap_or(0);
    let mut cache = ALT_CACHE.write().await;
    *cache = Some((alt_address, alt.clone(), current_slot));
    Ok(alt)
}

// Legacy full TX builders moved to super::builders_legacy
