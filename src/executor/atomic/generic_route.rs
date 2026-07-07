//! Generic DEX route builder — types, account helpers, resolvers, pricing.

use anyhow::Context;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use crate::config::AppConfig;
use crate::constants;
use crate::pool_cache;
use crate::simulator;

use super::onchain_router::{push_pumpswap_buy_accounts, push_pumpswap_sell_accounts, pumpswap_buy_remaining_count};

// ── Generic route builder (ROUTE_DISC) for CPMM / Whirlpool ─────────────

/// Pool data needed to build a Whirlpool DEX section.
///
/// Account layout (12 fixed, matches Orca IDL + token_program):
///   [0]=Whirlpool program, [1]=token_authority(PDA), [2]=whirlpool state,
///   [3]=user input ATA (filled by CPI), [4]=vault_a, [5]=user output ATA (filled),
///   [6]=vault_b, [7-9]=tick_arrays, [10]=oracle(PDA), [11]=token_program
///
/// Vaults are NEVER swapped — direction is controlled by `a_to_b` in the IX data.
/// On-chain orchestrator requires token_x = SOL (a_to_b=true for buy, false for sell).
pub(crate) struct WhirlpoolSectionData {
    pub pool: Pubkey,
    pub vault_a: Pubkey,  // token_x vault (must be SOL for orchestrator)
    pub vault_b: Pubkey,  // token_y vault (meme)
    pub tick_arrays: [Pubkey; 3],
    #[allow(dead_code)]
    pub sol_is_x: bool,   // must be true; false means unsupported pool
}

/// Fixed account count for a Whirlpool section (matches on-chain dex_whirlpool::WHIRLPOOL_FIXED_LEN).
#[allow(dead_code)]
const WHIRLPOOL_SECTION_FIXED: usize = 12;

/// Pool data needed to build a CPMM DEX section.
/// Vaults are PDA-derived in push_cpmm_accounts from pool + mints.
pub(crate) struct CpmmSectionData {
    pub pool: Pubkey,
    pub config: Pubkey,
}

/// Pool data needed to build a PumpSwap DEX section.
/// Always pushes 23 fixed accounts (sell pads to match buy layout).
pub(crate) struct PumpSwapSectionData {
    pub pool: Pubkey,
    pub meta: simulator::PumpSwapPoolMeta,
    pub remaining_count: u8,
}

/// Pool data needed to build a DLMM DEX section (extended: 13 fixed + bins).
/// Positions 9-12 carry token mints/programs for generic orchestrator CPI.
pub(crate) struct DlmmSectionData {
    pub program: Pubkey,
    pub lb_pair: Pubkey,
    pub bitmap: Pubkey,
    pub reserve_x: Pubkey,
    pub reserve_y: Pubkey,
    pub oracle: Pubkey,
    pub host_fee: Pubkey,
    pub memo: Pubkey,
    pub event_auth: Pubkey,
    pub token_x_mint: Pubkey,
    pub token_y_mint: Pubkey,
    pub token_x_program: Pubkey,
    pub token_y_program: Pubkey,
    pub bin_arrays: Vec<Pubkey>,
    pub bin_count: u8,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_generic_route(
    wallet_pubkey: &Pubkey,
    user_sol_ata: &Pubkey,
    user_meme_ata: &Pubkey,
    meme_mint: &Pubkey,
    sol_mint: &Pubkey,
    investment_lamports: u64,
    min_meme_out: u64,
    min_profit_lamports: u64,
    buy_kind: u8,
    sell_kind: u8,
    buy_cpmm: Option<&CpmmSectionData>,
    buy_whirlpool: Option<&WhirlpoolSectionData>,
    buy_pumpswap: Option<&PumpSwapSectionData>,
    buy_dlmm: Option<&DlmmSectionData>,
    sell_cpmm: Option<&CpmmSectionData>,
    sell_whirlpool: Option<&WhirlpoolSectionData>,
    sell_pumpswap: Option<&PumpSwapSectionData>,
    sell_dlmm: Option<&DlmmSectionData>,
    buy_remaining: u8,
    sell_remaining: u8,
    buy_sol_is_x: bool,
    meme_token_program: &Pubkey,
    sol_token_program: &Pubkey,
    config: &AppConfig,
) -> anyhow::Result<Instruction> {
    let arb_program_id = Pubkey::from_str(&config.execution_routing.onchain_program_id)?;
    let token22 = Pubkey::from_str(constants::TOKEN22_PROGRAM)?;
    let memo = Pubkey::from_str(constants::MEMO_PROGRAM)?;

    let mut accounts: Vec<AccountMeta> = Vec::new();

    // Shared [0..=2]
    accounts.push(AccountMeta::new(*wallet_pubkey, true));
    accounts.push(AccountMeta::new(*user_sol_ata, false));
    accounts.push(AccountMeta::new(*user_meme_ata, false));

    // Buy section (SOL→meme)
    match buy_kind {
        constants::DEX_KIND_PUMPSWAP => {
            let d = buy_pumpswap.context("missing PumpSwap buy data")?;
            push_pumpswap_generic_buy(
                &mut accounts, wallet_pubkey, user_sol_ata, user_meme_ata,
                meme_mint, sol_mint, meme_token_program, sol_token_program,
                &d.pool, &d.meta,
            );
        }
        constants::DEX_KIND_DLMM => {
            let d = buy_dlmm.context("missing DLMM buy data")?;
            push_dlmm_generic_accounts(&mut accounts, d);
        }
        constants::DEX_KIND_CPMM => {
            let d = buy_cpmm.context("missing CPMM buy data")?;
            push_cpmm_accounts(
                &mut accounts, &d.pool, sol_mint, meme_mint,
                user_sol_ata, user_meme_ata, sol_token_program, meme_token_program, &memo, &token22,
                &d.config,
            );
        }
        constants::DEX_KIND_WHIRLPOOL => {
            let d = buy_whirlpool.context("missing Whirlpool buy data")?;
            push_whirlpool_accounts(
                &mut accounts, &d.pool,
                user_sol_ata, user_meme_ata,
                &d.vault_a, &d.vault_b,
                &d.tick_arrays, d.tick_arrays.len(),
            );
        }
        _ => anyhow::bail!("unsupported buy DEX kind: {}", buy_kind),
    }

    // Sell section (meme→SOL)
    match sell_kind {
        constants::DEX_KIND_PUMPSWAP => {
            let d = sell_pumpswap.context("missing PumpSwap sell data")?;
            push_pumpswap_generic_sell(
                &mut accounts, wallet_pubkey, user_sol_ata, user_meme_ata,
                meme_mint, sol_mint, meme_token_program, sol_token_program,
                &d.pool, &d.meta,
            );
        }
        constants::DEX_KIND_DLMM => {
            let d = sell_dlmm.context("missing DLMM sell data")?;
            push_dlmm_generic_accounts(&mut accounts, d);
        }
        constants::DEX_KIND_CPMM => {
            let d = sell_cpmm.context("missing CPMM sell data")?;
            push_cpmm_accounts(
                &mut accounts, &d.pool, meme_mint, sol_mint,
                user_meme_ata, user_sol_ata, meme_token_program, sol_token_program, &memo, &token22,
                &d.config,
            );
        }
        constants::DEX_KIND_WHIRLPOOL => {
            let d = sell_whirlpool.context("missing Whirlpool sell data")?;
            // Vaults are never swapped — the on-chain orchestrator controls
            // direction via a_to_b flag. vault_a is token_x vault, vault_b is
            // token_y vault at fixed positions in the account layout.
            push_whirlpool_accounts(
                &mut accounts, &d.pool,
                user_meme_ata, user_sol_ata,
                &d.vault_a, &d.vault_b,
                &d.tick_arrays, d.tick_arrays.len(),
            );
        }
        _ => anyhow::bail!("unsupported sell DEX kind: {}", sell_kind),
    }

    // IX data (36 bytes): ROUTE_DISC + common fields.
    let ix_buy_remaining = match buy_kind {
        constants::DEX_KIND_WHIRLPOOL => 0u8,
        constants::DEX_KIND_PUMPSWAP => buy_remaining,
        constants::DEX_KIND_DLMM => buy_dlmm.map(|d| d.bin_count).unwrap_or(0),
        _ => buy_remaining,
    };
    let ix_sell_remaining = match sell_kind {
        constants::DEX_KIND_WHIRLPOOL => 0u8,
        constants::DEX_KIND_PUMPSWAP => sell_remaining,
        constants::DEX_KIND_DLMM => sell_dlmm.map(|d| d.bin_count).unwrap_or(0),
        _ => sell_remaining,
    };

    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&constants::ROUTE_DISC);
    data.extend_from_slice(&investment_lamports.to_le_bytes());
    data.extend_from_slice(&min_profit_lamports.to_le_bytes());
    data.extend_from_slice(&min_meme_out.to_le_bytes());
    data.push(0u8);  // track_volume
    data.push(buy_sol_is_x as u8);
    data.push(ix_buy_remaining);
    data.push(ix_sell_remaining);

    Ok(Instruction {
        program_id: arb_program_id,
        accounts,
        data,
    })
}

fn push_cpmm_accounts(
    accounts: &mut Vec<AccountMeta>,
    pool: &Pubkey,
    input_mint: &Pubkey,
    output_mint: &Pubkey,
    input_ata: &Pubkey,
    output_ata: &Pubkey,
    _input_tp: &Pubkey,
    _output_tp: &Pubkey,
    memo_program: &Pubkey,
    token22: &Pubkey,
    amm_config: &Pubkey,
) {
    let cpmm_prog = Pubkey::from_str(constants::CPMM_PROGRAM).unwrap();
    let (auth, _) = Pubkey::find_program_address(&[b"vault_and_lp", &pool.to_bytes()], &cpmm_prog);
    let (input_vault, _) = Pubkey::find_program_address(
        &[b"pool_vault", &pool.to_bytes(), &input_mint.to_bytes()], &cpmm_prog);
    let (output_vault, _) = Pubkey::find_program_address(
        &[b"pool_vault", &pool.to_bytes(), &output_mint.to_bytes()], &cpmm_prog);

    accounts.push(AccountMeta::new_readonly(cpmm_prog, false)); // 0
    accounts.push(AccountMeta::new_readonly(auth, false));      // 1
    accounts.push(AccountMeta::new_readonly(*amm_config, false)); // 2
    accounts.push(AccountMeta::new(*pool, false));              // 3
    accounts.push(AccountMeta::new(*input_ata, false));         // 4: user input ATA
    accounts.push(AccountMeta::new(*output_ata, false));        // 5: user output ATA
    accounts.push(AccountMeta::new(input_vault, false));        // 6
    accounts.push(AccountMeta::new(output_vault, false));       // 7
    accounts.push(AccountMeta::new_readonly(*input_mint, false)); // 8
    accounts.push(AccountMeta::new_readonly(*output_mint, false)); // 9
    accounts.push(AccountMeta::new_readonly(tokenkeg_cl(), false)); // 10
    accounts.push(AccountMeta::new_readonly(*token22, false));     // 11
    accounts.push(AccountMeta::new_readonly(*memo_program, false)); // 12
}

fn push_whirlpool_accounts(
    accounts: &mut Vec<AccountMeta>,
    pool: &Pubkey,
    input_ata: &Pubkey,
    output_ata: &Pubkey,
    vault_a: &Pubkey,
    vault_b: &Pubkey,
    tick_arrays: &[Pubkey],
    tick_count: usize,
) {
    let wp = Pubkey::from_str(constants::WHIRLPOOL_PROGRAM).unwrap();
    let (auth, _) = Pubkey::find_program_address(&[b"authority"], &wp);
    let (oracle, _) = Pubkey::find_program_address(&[b"oracle", &pool.to_bytes()], &wp);
    let tc = tick_count.min(3);

    accounts.push(AccountMeta::new_readonly(wp, false));        // 0: program
    accounts.push(AccountMeta::new_readonly(auth, true));       // 1: token_authority (PDA signer)
    accounts.push(AccountMeta::new(*pool, false));              // 2: whirlpool state
    // Positions 3/5: CPI builder reads user ATAs from input_ata_idx /
    // output_ata_idx (absolute indices), not from these section slots.
    // We store the actual ATA addresses so on-chain validation can
    // inspect them if needed.
    accounts.push(AccountMeta::new(*input_ata, false));         // 3: user input ATA
    accounts.push(AccountMeta::new(*vault_a, false));           // 4: vault_a
    accounts.push(AccountMeta::new(*output_ata, false));        // 5: user output ATA
    accounts.push(AccountMeta::new(*vault_b, false));           // 6: vault_b
    for i in 0..tc {
        let ta = tick_arrays.get(i).copied().unwrap_or(wp);
        accounts.push(AccountMeta::new(ta, false));             // 7-9: tick arrays
    }
    // Pad missing tick arrays with whirlpool program address
    for _ in tc..3 {
        accounts.push(AccountMeta::new(wp, false));
    }
    accounts.push(AccountMeta::new_readonly(oracle, false));    // 10: oracle
    accounts.push(AccountMeta::new_readonly(tokenkeg_cl(), false)); // 11: token_program
}

// ── PumpSwap generic section builders (reuse existing push helpers) ────

fn push_pumpswap_generic_buy(
    accounts: &mut Vec<AccountMeta>,
    wallet_pubkey: &Pubkey,
    user_sol_ata: &Pubkey,
    user_meme_ata: &Pubkey,
    meme_mint: &Pubkey,
    sol_mint: &Pubkey,
    meme_token_program: &Pubkey,
    sol_token_program: &Pubkey,
    pool: &Pubkey,
    meta: &simulator::PumpSwapPoolMeta,
) {
    push_pumpswap_buy_accounts(
        accounts, wallet_pubkey, user_sol_ata, user_meme_ata,
        meme_mint, sol_mint, meme_token_program, sol_token_program,
        pool, meta,
    );
}

fn push_pumpswap_generic_sell(
    accounts: &mut Vec<AccountMeta>,
    wallet_pubkey: &Pubkey,
    user_sol_ata: &Pubkey,
    user_meme_ata: &Pubkey,
    meme_mint: &Pubkey,
    sol_mint: &Pubkey,
    meme_token_program: &Pubkey,
    sol_token_program: &Pubkey,
    pool: &Pubkey,
    meta: &simulator::PumpSwapPoolMeta,
) {
    // Push base 21 sell accounts
    push_pumpswap_sell_accounts(
        accounts, wallet_pubkey, user_sol_ata, user_meme_ata,
        meme_mint, sol_mint, meme_token_program, sol_token_program,
        pool, meta,
    );
    // Pad to 23 fixed (generic orchestrator uses 23 for both buy/sell).
    // Positions 21-22 are unused by the sell CPI builder.
    let system_prog = Pubkey::from_str("11111111111111111111111111111111").unwrap();
    accounts.push(AccountMeta::new_readonly(system_prog, false)); // 21: pad
    accounts.push(AccountMeta::new_readonly(system_prog, false)); // 22: pad
}

// ── DLMM generic section builder (extended: 13 fixed + bins) ──────────

fn push_dlmm_generic_accounts(
    accounts: &mut Vec<AccountMeta>,
    d: &DlmmSectionData,
) {
    accounts.push(AccountMeta::new_readonly(d.program, false));      // 0
    accounts.push(AccountMeta::new(d.lb_pair, false));               // 1
    accounts.push(AccountMeta::new(d.bitmap, false));                // 2
    accounts.push(AccountMeta::new(d.reserve_x, false));             // 3
    accounts.push(AccountMeta::new(d.reserve_y, false));             // 4
    accounts.push(AccountMeta::new(d.oracle, false));                // 5
    accounts.push(AccountMeta::new(d.host_fee, false));              // 6
    accounts.push(AccountMeta::new_readonly(d.memo, false));         // 7
    accounts.push(AccountMeta::new_readonly(d.event_auth, false));   // 8
    accounts.push(AccountMeta::new_readonly(d.token_x_mint, false)); // 9
    accounts.push(AccountMeta::new_readonly(d.token_y_mint, false)); // 10
    accounts.push(AccountMeta::new_readonly(d.token_x_program, false)); // 11
    accounts.push(AccountMeta::new_readonly(d.token_y_program, false)); // 12
    for bin in &d.bin_arrays {
        accounts.push(AccountMeta::new(*bin, false));                // 13+
    }
}

fn tokenkeg_cl() -> Pubkey {
    Pubkey::from_str(constants::TOKEN_PROGRAM).unwrap()
}

// ── Whirlpool tick array derivation ─────────────────────────────────────

/// Tick array size from Orca Whirlpool protocol.
const WHIRLPOOL_TICK_ARRAY_SIZE: i32 = 88;

/// Derive tick array PDAs for a Whirlpool pool.
///
/// Returns up to 3 tick array addresses covering the swap range starting
/// from the array containing `tick_current_index`.
fn derive_whirlpool_tick_arrays(
    whirlpool: &Pubkey,
    tick_current_index: i32,
    tick_spacing: u16,
) -> Vec<Pubkey> {
    let wp = Pubkey::from_str(constants::WHIRLPOOL_PROGRAM).unwrap();
    let tick_range = tick_spacing as i32 * WHIRLPOOL_TICK_ARRAY_SIZE;
    if tick_range <= 0 {
        return vec![wp; 3]; // fallback for zero tick_spacing
    }

    // Floor division for signed integers (Rust / truncates toward zero)
    let start_tick = floor_div_i32(tick_current_index, tick_range) * tick_range;

    (0..3)
        .map(|i| {
            let tick = start_tick + i as i32 * tick_range;
            let (ta, _) = Pubkey::find_program_address(
                &[
                    b"tick_array",
                    &whirlpool.to_bytes(),
                    &tick.to_le_bytes(),
                ],
                &wp,
            );
            ta
        })
        .collect()
}

/// Floor division for i32 (Rust's built-in / truncates toward zero).
fn floor_div_i32(a: i32, b: i32) -> i32 {
    let d = a / b;
    let r = a % b;
    if r != 0 && (a ^ b) < 0 {
        d - 1
    } else {
        d
    }
}

// ── Pool section data resolvers ─────────────────────────────────────────

/// Build CPMM section data from pool cache `PoolStateData`.
pub(crate) fn cpmm_section_data(pool_state: &pool_cache::PoolStateData) -> anyhow::Result<CpmmSectionData> {
    let pool = Pubkey::from_str(&pool_state.pool_pubkey)?;
    let config = Pubkey::from_str(&pool_state.config_pubkey)
        .unwrap_or_else(|_| Pubkey::from_str(constants::CPMM_AMM_CONFIG).unwrap());
    Ok(CpmmSectionData { pool, config })
}

/// Build Whirlpool section data from cached reserves.
pub(crate) fn whirlpool_section_data(
    reserves: &pool_cache::DlmmPoolReserves,
) -> anyhow::Result<WhirlpoolSectionData> {
    let pool = Pubkey::from_str(&reserves.lb_pair)?;
    let vault_a = Pubkey::from_str(&reserves.reserve_x_address)?;
    let vault_b = Pubkey::from_str(&reserves.reserve_y_address)?;

    // On-chain orchestrator uses a_to_b=true for buy, a_to_b=false for sell,
    // requiring SOL = token_x. Pools where SOL = token_y are unsupported.
    let sol_is_x = reserves.token_x_mint == constants::NATIVE_SOL_MINT;
    anyhow::ensure!(
        sol_is_x,
        "Whirlpool pool {} has SOL as token_y — unsupported by current orchestrator",
        &reserves.lb_pair[..16]
    );

    let tick_array_vec = derive_whirlpool_tick_arrays(
        &pool,
        reserves.tick_current_index,
        reserves.bin_step,
    );
    let tick_arrays: [Pubkey; 3] = [
        *tick_array_vec.first().unwrap_or(&pool),
        *tick_array_vec.get(1).unwrap_or(&pool),
        *tick_array_vec.get(2).unwrap_or(&pool),
    ];
    Ok(WhirlpoolSectionData { pool, vault_a, vault_b, tick_arrays, sol_is_x })
}

// ── Pricing helpers (CPMM / Whirlpool subtractive fee model) ──────────

/// Fee rate denominator for Raydium CPMM and Orca Whirlpool (1_000_000).
/// CPMM: trade_fee_rate / 1_000_000 = fee % (2500 = 0.25%).
/// Whirlpool: fee_rate / 1_000_000 = fee % (3000 = 0.30%).
/// Verified against official Orca Whirlpool rust-sdk `try_apply_swap_fee`
/// (core/src/math/token.rs:321) and Raydium CPMM docs.
const SWAP_FEE_DENOM: u128 = 1_000_000;
/// Default CPMM trade_fee_rate for AmmConfig[0] = 0.25%.
pub(crate) const CPMM_DEFAULT_TRADE_FEE_RATE: u64 = 2500;

/// Constant-product swap output with subtractive fee model.
///
/// Formula matches Orca `try_apply_swap_fee` and Raydium CPMM:
///   amount_after_fee = amount_in * (FEE_RATE_DENOM - fee_rate) / FEE_RATE_DENOM
///   output = reserve_out * amount_after_fee / (reserve_in + amount_after_fee)
///
/// Rounding: amount_after_fee rounds down (conservative for pool), output rounds down.
/// Returns None on overflow or zero output.
pub(crate) fn checked_cp_swap_output(
    reserve_in: u64,
    reserve_out: u64,
    amount_in: u64,
    fee_rate: u64,       // numerator, denominator = SWAP_FEE_DENOM (1_000_000)
) -> Option<u64> {
    if amount_in == 0 || reserve_out == 0 {
        return Some(0);
    }
    let ri = reserve_in as u128;
    let ro = reserve_out as u128;
    let a = amount_in as u128;
    let rate = fee_rate as u128;

    // amount_after_fee = amount * (DENOM - rate) / DENOM  (round down, matches Orca SDK)
    let product = SWAP_FEE_DENOM.checked_sub(rate)?;
    let numerator = a.checked_mul(product)?;
    let amount_after = numerator.checked_div(SWAP_FEE_DENOM)?;
    if amount_after == 0 {
        return None;
    }
    // output = reserve_out * amount_after / (reserve_in + amount_after)  (round down)
    let num = ro.checked_mul(amount_after)?;
    let den = ri.checked_add(amount_after)?;
    if den == 0 {
        return None;
    }
    let out = num.checked_div(den)?;
    if out == 0 {
        return None;
    }
    Some(out as u64)
}

// ── Section data resolvers for PumpSwap / DLMM ─────────────────────────

pub(crate) fn dlmm_section_data(dlmm: &pool_cache::DlmmPoolReserves, meme_token_program: &Pubkey) -> anyhow::Result<DlmmSectionData> {
    let program = Pubkey::from_str(constants::DLMM_PROGRAM)?;
    let lb_pair = Pubkey::from_str(&dlmm.lb_pair)?;
    let bitmap = dlmm.bin_array_bitmap_extension.as_deref()
        .and_then(|s| Pubkey::from_str(s).ok()).unwrap_or(program);
    let reserve_x = Pubkey::from_str(&dlmm.reserve_x_address)?;
    let reserve_y = Pubkey::from_str(&dlmm.reserve_y_address)?;
    let (oracle, _) = Pubkey::find_program_address(&[b"oracle", &lb_pair.to_bytes()], &program);
    let memo = Pubkey::from_str(constants::MEMO_PROGRAM)?;
    let event_auth = Pubkey::from_str(constants::DLMM_EVENT_AUTHORITY)?;
    let token_x_mint = Pubkey::from_str(&dlmm.token_x_mint)?;
    let token_y_mint = Pubkey::from_str(&dlmm.token_y_mint)?;
    let tokenkeg = tokenkeg_cl();
    let bin_count = dlmm.bin_array_addresses.len().min(5);
    let bin_arrays: Vec<Pubkey> = dlmm.bin_array_addresses.iter().take(bin_count)
        .map(|a| Pubkey::from_str(a).unwrap()).collect();
    Ok(DlmmSectionData {
        program, lb_pair, bitmap, reserve_x, reserve_y, oracle,
        host_fee: program, memo, event_auth,
        token_x_mint, token_y_mint,
        token_x_program: tokenkeg, token_y_program: *meme_token_program,
        bin_arrays, bin_count: bin_count as u8,
    })
}

pub(crate) fn pumpswap_section_data(
    meta: &simulator::PumpSwapPoolMeta, pool: &Pubkey,
) -> anyhow::Result<PumpSwapSectionData> {
    Ok(PumpSwapSectionData {
        pool: *pool, meta: meta.clone(),
        remaining_count: pumpswap_buy_remaining_count(meta),
    })
}

