//! Shared test utilities for arbitrage integration tests.
#![allow(dead_code)]

use solana_program::pubkey::Pubkey;
use solana_program_test::{ProgramTest, ProgramTestContext};
use solana_sdk::{account::Account, bpf_loader, signature::Keypair, signer::Signer};

// ── Program / stub .so paths (relative to Cargo.toml dir) ───────────────
const ARBITRAGE_SO: &str = "target/deploy/arbitrage.so";
const STUB_DLMM_SO: &str = "tests/stubs/stub_dlmm.so";
pub const STUB_PUMP_SWAP_SO: &str = "tests/stubs/stub_pump_swap.so";
pub const STUB_PUMP_SWAP_PARTIAL_SO: &str = "tests/stubs/stub_pump_swap_partial.so";

// ── Program IDs ─────────────────────────────────────────────────────────
pub const ARBITRAGE_ID: Pubkey =
    solana_program::pubkey!("ARB1trage11111111111111111111111111111111111");
pub const PUMP_SWAP_ID: Pubkey =
    solana_program::pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
pub const DLMM_ID: Pubkey = solana_program::pubkey!("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo");
const TOKEN_ID: Pubkey = solana_program::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const SYSTEM_ID: Pubkey = solana_program::pubkey!("11111111111111111111111111111111");
const ATA_ID: Pubkey = solana_program::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const FEE_PROGRAM_ID: Pubkey =
    solana_program::pubkey!("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
const MEMO_ID: Pubkey = solana_program::pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
pub const DLMM_EVENT_AUTH: Pubkey =
    solana_program::pubkey!("D1ZN9Wj1fRSUQfCjhvnu1hqDMT7hzjzBBpi12nVniYD6");
pub const NATIVE_SOL_MINT: Pubkey =
    solana_program::pubkey!("So11111111111111111111111111111111111111112");

// ── PDA seeds ───────────────────────────────────────────────────────────
const PUMP_EVENT_AUTH_SEED: &[u8] = b"__event_authority";
const PUMP_GLOBAL_CONFIG_SEED: &[u8] = b"global_config";
const PUMP_FEE_CONFIG_SEED: &[u8] = b"fee_config";
const DLMM_ORACLE_SEED: &[u8] = b"oracle";

// ── Instruction data layout ─────────────────────────────────────────────
pub const ROUTE_PUMP_TO_DLMM_DISC: [u8; 8] = [0x8b, 0xe8, 0x20, 0x55, 0xc1, 0xb0, 0xc1, 0xe9];
pub const ROUTE_DLMM_TO_PUMP_DISC: [u8; 8] = [0x17, 0x6e, 0xcc, 0x5d, 0xdd, 0x93, 0x51, 0x95];
const OFF_AMOUNT_IN: usize = 8;
const OFF_MIN_PROFIT: usize = 16;
const OFF_MIN_INTERMEDIATE: usize = 24;
const OFF_TRACK_VOLUME: usize = 32;
const OFF_DLMM_SOL_IS_X: usize = 33;
const OFF_PUMP_REMAINING: usize = 34;
const OFF_DLMM_BIN_ARRAY_COUNT: usize = 35;
const IX_DATA_LEN: usize = 36;

// ── Account index constants ─────────────────────────────────────────────
pub const USER_IDX: usize = 0;
pub const USER_SOL_ATA_IDX: usize = 1;
pub const USER_MEME_ATA_IDX: usize = 2;
pub const SHARED_FIXED_LEN: usize = 3;
pub const DLMM_FIXED_LEN: usize = 9;
pub const PUMP_SELL_FIXED_LEN: usize = 21;

// DLMM relative offsets
const DLMM_LB_PAIR_REL: usize = 1;
const DLMM_ORACLE_REL: usize = 5;
const DLMM_MEMO_REL: usize = 7;
const DLMM_EVENT_AUTH_REL: usize = 8;

// PumpSwap Sell relative offsets
const PUMP_SELL_QUOTE_MINT: usize = 4;
const PUMP_SELL_QUOTE_TOKEN_PROGRAM: usize = 12;
const PUMP_SELL_SYSTEM_PROGRAM: usize = 13;
const PUMP_SELL_ATA_PROGRAM: usize = 14;
const PUMP_SELL_EVENT_AUTHORITY: usize = 15;
const PUMP_SELL_PROGRAM: usize = 16;
const PUMP_SELL_FEE_CONFIG: usize = 19;
const PUMP_SELL_FEE_PROGRAM: usize = 20;

// ── PDA derivation helpers ──────────────────────────────────────────────

fn pda_pump_event_auth() -> Pubkey {
    Pubkey::find_program_address(&[PUMP_EVENT_AUTH_SEED], &PUMP_SWAP_ID).0
}

fn pda_pump_global_config() -> Pubkey {
    Pubkey::find_program_address(&[PUMP_GLOBAL_CONFIG_SEED], &PUMP_SWAP_ID).0
}

fn pda_pump_fee_config() -> Pubkey {
    Pubkey::find_program_address(
        &[PUMP_FEE_CONFIG_SEED, PUMP_SWAP_ID.as_ref()],
        &FEE_PROGRAM_ID,
    )
    .0
}

fn pda_dlmm_oracle(lb_pair: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[DLMM_ORACLE_SEED, lb_pair.as_ref()], &DLMM_ID).0
}

fn pda_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), &TOKEN_ID.as_ref(), mint.as_ref()],
        &ATA_ID,
    )
    .0
}

// ── Account factories ───────────────────────────────────────────────────

fn sys_account(lamports: u64) -> Account {
    Account::new(lamports, 0, &SYSTEM_ID)
}

fn token_account(mint: &Pubkey, owner: &Pubkey, amount: u64) -> Account {
    let mut data = vec![0u8; 165];
    data[0..32].copy_from_slice(&mint.to_bytes());
    data[32..64].copy_from_slice(&owner.to_bytes());
    data[64..72].copy_from_slice(&amount.to_le_bytes());
    data[108] = 1; // State: Initialized
                   // Rent-exempt: ~1.2 SOL covers 165 bytes comfortably
    Account {
        lamports: 2_000_000_000,
        data,
        owner: TOKEN_ID,
        executable: false,
        rent_epoch: u64::MAX,
    }
}

pub fn build_ix_data(
    route_disc: [u8; 8],
    amount_in: u64,
    min_profit_lamports: u64,
    min_intermediate_meme: u64,
    track_volume: bool,
    dlmm_sol_is_x: bool,
    pump_remaining_count: u8,
    dlmm_bin_array_count: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; IX_DATA_LEN];
    buf[0..8].copy_from_slice(&route_disc);
    buf[OFF_AMOUNT_IN..OFF_AMOUNT_IN + 8].copy_from_slice(&amount_in.to_le_bytes());
    buf[OFF_MIN_PROFIT..OFF_MIN_PROFIT + 8].copy_from_slice(&min_profit_lamports.to_le_bytes());
    buf[OFF_MIN_INTERMEDIATE..OFF_MIN_INTERMEDIATE + 8]
        .copy_from_slice(&min_intermediate_meme.to_le_bytes());
    buf[OFF_TRACK_VOLUME] = track_volume as u8;
    buf[OFF_DLMM_SOL_IS_X] = dlmm_sol_is_x as u8;
    buf[OFF_PUMP_REMAINING] = pump_remaining_count;
    buf[OFF_DLMM_BIN_ARRAY_COUNT] = dlmm_bin_array_count;
    buf
}

// ── Test context ────────────────────────────────────────────────────────

pub struct TestAddresses {
    pub user: Keypair,
    pub user_sol_ata: Pubkey,
    pub user_meme_ata: Pubkey,
    pub meme_mint: Pubkey,
    pub lb_pair: Pubkey,
    pub dlmm_oracle: Pubkey,
    pub pump_pool: Pubkey,
    pub pump_event_auth: Pubkey,
    pub pump_global_config: Pubkey,
    pub pump_fee_config: Pubkey,
}

impl TestAddresses {
    pub fn new() -> Self {
        let user = Keypair::new();
        let meme_mint = Keypair::new().pubkey();
        let lb_pair = Keypair::new().pubkey();
        let pump_pool = Keypair::new().pubkey();

        Self {
            user_sol_ata: pda_ata(&user.pubkey(), &NATIVE_SOL_MINT),
            user_meme_ata: pda_ata(&user.pubkey(), &meme_mint),
            dlmm_oracle: pda_dlmm_oracle(&lb_pair),
            pump_event_auth: pda_pump_event_auth(),
            pump_global_config: pda_pump_global_config(),
            pump_fee_config: pda_pump_fee_config(),
            user,
            meme_mint,
            lb_pair,
            pump_pool,
        }
    }
}

/// Set up ProgramTest for route_dlmm_to_pump.
/// Account layout: [shared 3] [DLMM 9 + bins] [PumpSwap Sell 21 + remaining]
pub async fn setup_dlmm_to_pump(
    a: &TestAddresses,
    dlmm_bin_array_count: u8,
    amount_in: u64,
    preload_meme: u64,
    pump_swap_so: &str,
) -> (ProgramTestContext, Vec<Pubkey>) {
    let mut pt = ProgramTest::default();

    // Deploy BPF programs (solana-program-test 1.18 uses Account-based loading)
    let load_bpf = |path: &str| -> Vec<u8> {
        std::fs::read(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e))
    };

    pt.add_account(
        ARBITRAGE_ID,
        Account {
            lamports: 1_000_000_000,
            data: load_bpf(ARBITRAGE_SO),
            owner: bpf_loader::id(),
            executable: true,
            rent_epoch: 0,
        },
    );
    pt.add_account(
        DLMM_ID,
        Account {
            lamports: 1_000_000_000,
            data: load_bpf(STUB_DLMM_SO),
            owner: bpf_loader::id(),
            executable: true,
            rent_epoch: 0,
        },
    );
    pt.add_account(
        PUMP_SWAP_ID,
        Account {
            lamports: 1_000_000_000,
            data: load_bpf(pump_swap_so),
            owner: bpf_loader::id(),
            executable: true,
            rent_epoch: 0,
        },
    );

    let mut order: Vec<Pubkey> = Vec::new();

    // ── Shared accounts [0..2] ─────────────────────────────────────────
    let user_pk = a.user.pubkey();
    order.push(user_pk);
    pt.add_account(user_pk, sys_account(50_000_000)); // 0.05 SOL

    order.push(a.user_sol_ata);
    pt.add_account(
        a.user_sol_ata,
        token_account(&NATIVE_SOL_MINT, &user_pk, amount_in + 2_000_000),
    );

    order.push(a.user_meme_ata);
    pt.add_account(
        a.user_meme_ata,
        token_account(&a.meme_mint, &user_pk, preload_meme),
    );

    // ── DLMM section (9 fixed + bin arrays) ───────────────────────────
    let _dlmm_base = order.len();

    // dlmm_base+0: program
    order.push(DLMM_ID);

    // dlmm_base+1: lb_pair
    order.push(a.lb_pair);
    pt.add_account(a.lb_pair, sys_account(0));

    // dlmm_base+2: bin_array_bitmap
    let bitmap = Keypair::new().pubkey();
    order.push(bitmap);
    pt.add_account(bitmap, sys_account(0));

    // dlmm_base+3: reserve_x (authority = user so stub can CPI Token transfer)
    let reserve_x = Keypair::new().pubkey();
    order.push(reserve_x);
    pt.add_account(
        reserve_x,
        token_account(&NATIVE_SOL_MINT, &user_pk, 1_000_000_000),
    );

    // dlmm_base+4: reserve_y (authority = user so stub can CPI Token transfer)
    let reserve_y = Keypair::new().pubkey();
    order.push(reserve_y);
    pt.add_account(
        reserve_y,
        token_account(&a.meme_mint, &user_pk, 1_000_000_000),
    );

    // dlmm_base+5: oracle PDA
    order.push(a.dlmm_oracle);
    pt.add_account(a.dlmm_oracle, sys_account(0));

    // dlmm_base+6: host_fee_in
    let host_fee = Keypair::new().pubkey();
    order.push(host_fee);
    pt.add_account(host_fee, sys_account(0));

    // dlmm_base+7: memo
    order.push(MEMO_ID);

    // dlmm_base+8: event_auth
    order.push(DLMM_EVENT_AUTH);

    // bin arrays
    for _ in 0..dlmm_bin_array_count {
        let bin = Keypair::new().pubkey();
        order.push(bin);
        pt.add_account(bin, sys_account(0));
    }

    // ── PumpSwap Sell section (21 fixed + remaining) ──────────────────
    let _ps_base = order.len();

    // ps+0: pool
    order.push(a.pump_pool);
    pt.add_account(a.pump_pool, sys_account(0));

    // ps+1: user
    order.push(user_pk);

    // ps+2: global_config PDA
    order.push(a.pump_global_config);
    pt.add_account(a.pump_global_config, sys_account(0));

    // ps+3: base_mint (meme)
    order.push(a.meme_mint);

    // ps+4: quote_mint (NATIVE_SOL_MINT)
    order.push(NATIVE_SOL_MINT);

    // ps+5: user_base_ata (= meme ATA, index 2)
    order.push(a.user_meme_ata);

    // ps+6: user_quote_ata (= WSOL ATA, index 1)
    order.push(a.user_sol_ata);

    // ps+7: pool_base_ata (authority = user so stub can CPI Token transfer)
    let pool_base_ata = pda_ata(&a.pump_pool, &a.meme_mint);
    order.push(pool_base_ata);
    pt.add_account(
        pool_base_ata,
        token_account(&a.meme_mint, &user_pk, 1_000_000_000),
    );

    // ps+8: pool_quote_ata (authority = user so stub can CPI Token transfer)
    let pool_quote_ata = pda_ata(&a.pump_pool, &NATIVE_SOL_MINT);
    order.push(pool_quote_ata);
    pt.add_account(
        pool_quote_ata,
        token_account(&NATIVE_SOL_MINT, &user_pk, 1_000_000_000),
    );

    // ps+9: protocol_fee_recipient
    let fee_recipient = Keypair::new().pubkey();
    order.push(fee_recipient);
    pt.add_account(fee_recipient, sys_account(0));

    // ps+10: protocol_fee_ata
    let fee_ata = pda_ata(&fee_recipient, &a.meme_mint);
    order.push(fee_ata);
    pt.add_account(fee_ata, token_account(&a.meme_mint, &fee_recipient, 0));

    // ps+11: base_token_program (Token)
    order.push(TOKEN_ID);

    // ps+12: quote_token_program (Token)
    order.push(TOKEN_ID);

    // ps+13: system_program
    order.push(SYSTEM_ID);

    // ps+14: ata_program
    order.push(ATA_ID);

    // ps+15: event_authority PDA
    order.push(a.pump_event_auth);
    pt.add_account(a.pump_event_auth, sys_account(0));

    // ps+16: program (PUMP_SWAP_ID)
    order.push(PUMP_SWAP_ID);

    // ps+17: coin_creator_vault_ata
    let creator_vault_auth = Keypair::new().pubkey();
    let creator_vault_ata = pda_ata(&creator_vault_auth, &a.meme_mint);
    order.push(creator_vault_ata);
    pt.add_account(
        creator_vault_ata,
        token_account(&a.meme_mint, &creator_vault_auth, 0),
    );

    // ps+18: coin_creator_vault_auth
    order.push(creator_vault_auth);
    pt.add_account(creator_vault_auth, sys_account(0));

    // ps+19: fee_config PDA
    order.push(a.pump_fee_config);
    pt.add_account(a.pump_fee_config, sys_account(0));

    // ps+20: fee_program
    order.push(FEE_PROGRAM_ID);

    // Pump remaining = 0 (no extra)

    // Add well-known executable programs (native/system programs are pre-deployed by ProgramTest).
    // Only DLMM and PumpSwap stubs need explicit BPF deployment — they are custom programs
    // that the arbitrage program CPIs into. Native programs (Token, System, ATA, Memo, Fee)
    // are already provided by the runtime.
    // NATIVE_SOL_MINT, meme_mint are pre-deployed or handled by the runtime.
    // DLMM_EVENT_AUTH is a well-known account key.
    pt.add_account(
        a.meme_mint,
        Account {
            lamports: 1_000_000_000,
            data: vec![0u8; 82],
            owner: TOKEN_ID,
            executable: false,
            rent_epoch: 0,
        },
    );

    let ctx = pt.start_with_context().await;
    (ctx, order)
}

/// Set up ProgramTest for route_pump_to_dlmm.
/// Account layout: [shared 3] [PumpSwap Buy 23 + remaining] [DLMM 9 + bins]
pub async fn setup_pump_to_dlmm(
    a: &TestAddresses,
    dlmm_bin_array_count: u8,
    amount_in: u64,
    preload_meme: u64,
) -> (ProgramTestContext, Vec<Pubkey>) {
    let mut pt = ProgramTest::default();

    let load_bpf = |path: &str| -> Vec<u8> {
        std::fs::read(path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e))
    };

    // Deploy BPF programs
    pt.add_account(
        ARBITRAGE_ID,
        Account {
            lamports: 1_000_000_000,
            data: load_bpf(ARBITRAGE_SO),
            owner: bpf_loader::id(),
            executable: true,
            rent_epoch: 0,
        },
    );
    pt.add_account(
        DLMM_ID,
        Account {
            lamports: 1_000_000_000,
            data: load_bpf(STUB_DLMM_SO),
            owner: bpf_loader::id(),
            executable: true,
            rent_epoch: 0,
        },
    );
    pt.add_account(
        PUMP_SWAP_ID,
        Account {
            lamports: 1_000_000_000,
            data: load_bpf(STUB_PUMP_SWAP_SO),
            owner: bpf_loader::id(),
            executable: true,
            rent_epoch: 0,
        },
    );

    let mut order: Vec<Pubkey> = Vec::new();
    let user_pk = a.user.pubkey();

    // ── Shared accounts [0..2] ────────────────────────────────────────
    order.push(user_pk);
    pt.add_account(user_pk, sys_account(50_000_000));
    order.push(a.user_sol_ata);
    pt.add_account(
        a.user_sol_ata,
        token_account(&NATIVE_SOL_MINT, &user_pk, amount_in + 2_000_000),
    );
    order.push(a.user_meme_ata);
    pt.add_account(
        a.user_meme_ata,
        token_account(&a.meme_mint, &user_pk, preload_meme),
    );

    // ── PumpSwap Buy section (23 fixed) ──────────────────────────────
    let _pump_buy_base = order.len();

    // 0: pool
    order.push(a.pump_pool);
    pt.add_account(a.pump_pool, sys_account(0));

    // 1: user (shared index 0)
    order.push(user_pk);

    // 2: global_config PDA
    order.push(a.pump_global_config);
    pt.add_account(a.pump_global_config, sys_account(0));

    // 3: base_mint (meme)
    order.push(a.meme_mint);

    // 4: quote_mint (NATIVE_SOL_MINT)
    order.push(NATIVE_SOL_MINT);

    // 5: user_base_ata (= meme ATA, shared index 2)
    order.push(a.user_meme_ata);

    // 6: user_quote_ata (= WSOL ATA, shared index 1)
    order.push(a.user_sol_ata);

    // 7: pool_base_ata
    let pool_base_ata = pda_ata(&a.pump_pool, &a.meme_mint);
    order.push(pool_base_ata);
    pt.add_account(
        pool_base_ata,
        token_account(&a.meme_mint, &user_pk, 1_000_000_000),
    );

    // 8: pool_quote_ata
    let pool_quote_ata = pda_ata(&a.pump_pool, &NATIVE_SOL_MINT);
    order.push(pool_quote_ata);
    pt.add_account(
        pool_quote_ata,
        token_account(&NATIVE_SOL_MINT, &user_pk, 1_000_000_000),
    );

    // 9: protocol_fee_recipient
    let fee_recipient = Keypair::new().pubkey();
    order.push(fee_recipient);
    pt.add_account(fee_recipient, sys_account(0));

    // 10: protocol_fee_ata
    let fee_ata = pda_ata(&fee_recipient, &a.meme_mint);
    order.push(fee_ata);
    pt.add_account(fee_ata, token_account(&a.meme_mint, &fee_recipient, 0));

    // 11: base_token_program
    order.push(TOKEN_ID);

    // 12: quote_token_program
    order.push(TOKEN_ID);

    // 13: system_program
    order.push(SYSTEM_ID);

    // 14: ata_program
    order.push(ATA_ID);

    // 15: event_authority PDA
    order.push(a.pump_event_auth);
    pt.add_account(a.pump_event_auth, sys_account(0));

    // 16: program (PUMP_SWAP_ID)
    order.push(PUMP_SWAP_ID);

    // 17: coin_creator_vault_ata
    let creator_vault_auth = Keypair::new().pubkey();
    let creator_vault_ata = pda_ata(&creator_vault_auth, &a.meme_mint);
    order.push(creator_vault_ata);
    pt.add_account(
        creator_vault_ata,
        token_account(&a.meme_mint, &creator_vault_auth, 0),
    );

    // 18: coin_creator_vault_auth
    order.push(creator_vault_auth);
    pt.add_account(creator_vault_auth, sys_account(0));

    // 19: global_vol_accum (buy-only account)
    let global_vol_accum = Keypair::new().pubkey();
    order.push(global_vol_accum);
    pt.add_account(global_vol_accum, sys_account(0));

    // 20: user_vol_accum (buy-only account)
    let user_vol_accum = Keypair::new().pubkey();
    order.push(user_vol_accum);
    pt.add_account(user_vol_accum, sys_account(0));

    // 21: fee_config PDA
    order.push(a.pump_fee_config);
    pt.add_account(a.pump_fee_config, sys_account(0));

    // 22: fee_program
    order.push(FEE_PROGRAM_ID);

    // ── DLMM section (9 fixed + bin arrays) ──────────────────────────
    let _dlmm_base = order.len();

    // dlmm+0: program
    order.push(DLMM_ID);

    // dlmm+1: lb_pair
    order.push(a.lb_pair);
    pt.add_account(a.lb_pair, sys_account(0));

    // dlmm+2: bin_array_bitmap
    let bitmap = Keypair::new().pubkey();
    order.push(bitmap);
    pt.add_account(bitmap, sys_account(0));

    // dlmm+3: reserve_x (receives user_token_in = meme, so must be meme mint)
    let reserve_x = Keypair::new().pubkey();
    order.push(reserve_x);
    pt.add_account(
        reserve_x,
        token_account(&a.meme_mint, &user_pk, 1_000_000_000),
    );

    // dlmm+4: reserve_y (pays user_token_out = WSOL, so must be SOL mint)
    let reserve_y = Keypair::new().pubkey();
    order.push(reserve_y);
    pt.add_account(
        reserve_y,
        token_account(&NATIVE_SOL_MINT, &user_pk, 1_000_000_000),
    );

    // dlmm+5: oracle
    order.push(a.dlmm_oracle);
    pt.add_account(a.dlmm_oracle, sys_account(0));

    // dlmm+6: host_fee_in
    let host_fee = Keypair::new().pubkey();
    order.push(host_fee);
    pt.add_account(host_fee, sys_account(0));

    // dlmm+7: memo
    order.push(MEMO_ID);

    // dlmm+8: event_auth
    order.push(DLMM_EVENT_AUTH);

    // bin arrays
    for _ in 0..dlmm_bin_array_count {
        let bin = Keypair::new().pubkey();
        order.push(bin);
        pt.add_account(bin, sys_account(0));
    }

    // Additional accounts
    pt.add_account(
        a.meme_mint,
        Account {
            lamports: 1_000_000_000,
            data: vec![0u8; 82],
            owner: TOKEN_ID,
            executable: false,
            rent_epoch: 0,
        },
    );

    let ctx = pt.start_with_context().await;
    (ctx, order)
}
