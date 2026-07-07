use super::*;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

fn test_pubkey(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

fn tk_prog() -> Pubkey {
    Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()
}

#[test]
fn buy_ix_account_count_classic_pool() {
    // coin_creator == default, is_cashback_coin == false → 23 fixed + 2 remaining = 25
    let ix = build_pumpswap_buy_ix(
        &test_pubkey(1),
        &test_pubkey(2),
        &test_pubkey(3),
        &Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
        &test_pubkey(5),
        &test_pubkey(6),
        &test_pubkey(7),
        &test_pubkey(8),
        &tk_prog(),
        &tk_prog(),
        1_000_000,
        500_000,
        true,
        &Pubkey::default(),
        false,
        &test_pubkey(99),
        &test_pubkey(88),
    );
    assert_eq!(
        ix.accounts.len(),
        25,
        "classic pool: 23 fixed + 2 remaining"
    );
    assert_eq!(ix.data.len(), 25, "buy_exact_quote_in data = 25 bytes");
}

#[test]
fn buy_ix_account_count_creator_pool() {
    // coin_creator != default → +1 pool_v2_pda = 26
    let ix = build_pumpswap_buy_ix(
        &test_pubkey(1),
        &test_pubkey(2),
        &test_pubkey(3),
        &Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
        &test_pubkey(5),
        &test_pubkey(6),
        &test_pubkey(7),
        &test_pubkey(8),
        &tk_prog(),
        &tk_prog(),
        1_000_000,
        500_000,
        true,
        &test_pubkey(10),
        false,
        &test_pubkey(99),
        &test_pubkey(88),
    );
    assert_eq!(ix.accounts.len(), 26, "creator pool: +pool_v2_pda");
}

#[test]
fn buy_ix_account_count_cashback_creator_pool() {
    // is_cashback_coin && coin_creator != default → +2 remaining = 27
    let ix = build_pumpswap_buy_ix(
        &test_pubkey(1),
        &test_pubkey(2),
        &test_pubkey(3),
        &Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
        &test_pubkey(5),
        &test_pubkey(6),
        &test_pubkey(7),
        &test_pubkey(8),
        &tk_prog(),
        &tk_prog(),
        1_000_000,
        500_000,
        true,
        &test_pubkey(10),
        true,
        &test_pubkey(99),
        &test_pubkey(88),
    );
    assert_eq!(
        ix.accounts.len(),
        27,
        "cashback + creator: 23 fixed + 4 remaining"
    );
}

#[test]
fn sell_ix_account_count_classic_pool() {
    // coin_creator == default, is_cashback_coin == false → 21 fixed + 2 remaining = 23
    let ix = build_pumpswap_sell_ix(
        &test_pubkey(1),
        &test_pubkey(2),
        &test_pubkey(3),
        &Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
        &test_pubkey(5),
        &test_pubkey(6),
        &test_pubkey(7),
        &test_pubkey(8),
        &tk_prog(),
        &tk_prog(),
        500_000,
        1_000_000,
        &Pubkey::default(),
        false,
        &test_pubkey(99),
        &test_pubkey(88),
    );
    assert_eq!(
        ix.accounts.len(),
        23,
        "classic sell: 21 fixed + 2 remaining"
    );
    assert_eq!(ix.data.len(), 24, "sell data = 24 bytes");
}

#[test]
fn sell_ix_account_count_cashback_creator_pool() {
    // is_cashback_coin && coin_creator != default → +3 remaining = 26
    let ix = build_pumpswap_sell_ix(
        &test_pubkey(1),
        &test_pubkey(2),
        &test_pubkey(3),
        &Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
        &test_pubkey(5),
        &test_pubkey(6),
        &test_pubkey(7),
        &test_pubkey(8),
        &tk_prog(),
        &tk_prog(),
        500_000,
        1_000_000,
        &test_pubkey(10),
        true,
        &test_pubkey(99),
        &test_pubkey(88),
    );
    assert_eq!(
        ix.accounts.len(),
        26,
        "cashback+creator sell: 21 fixed + 5 remaining"
    );
}

#[test]
fn buy_data_layout_matches_idl() {
    let ix = build_pumpswap_buy_ix(
        &test_pubkey(1),
        &test_pubkey(2),
        &test_pubkey(3),
        &Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap(),
        &test_pubkey(5),
        &test_pubkey(6),
        &test_pubkey(7),
        &test_pubkey(8),
        &tk_prog(),
        &tk_prog(),
        50_000_000,
        564_953_706,
        true,
        &Pubkey::default(),
        false,
        &test_pubkey(99),
        &test_pubkey(88),
    );
    assert_eq!(ix.data.len(), 25);
    assert_eq!(&ix.data[0..8], &PUMPSWAP_BUY_DISCRIMINATOR);
    assert_eq!(
        u64::from_le_bytes(ix.data[8..16].try_into().unwrap()),
        50_000_000
    );
    assert_eq!(
        u64::from_le_bytes(ix.data[16..24].try_into().unwrap()),
        564_953_706
    );
    assert_eq!(ix.data[24], 1);
}

#[test]
fn estimate_pumpswap_buy_obeys_fee() {
    let out_0bps =
        estimate_pumpswap_buy_output(1_000_000_000, 100_000_000_000, 1_000_000_000_000, 0);
    let out_25bps =
        estimate_pumpswap_buy_output(1_000_000_000, 100_000_000_000, 1_000_000_000_000, 25);
    let out_100bps =
        estimate_pumpswap_buy_output(1_000_000_000, 100_000_000_000, 1_000_000_000_000, 100);
    assert!(out_100bps < out_25bps, "higher fee = less output");
    assert!(out_25bps < out_0bps, "fee reduces output vs zero-fee");
}

#[test]
fn parse_pool_meta_real_data() {
    // Simulate pool account data: 8-byte discriminator + Pool struct (packed).
    let mut data = vec![0u8; 300];
    data[0..8].copy_from_slice(&[0xbc, 0xdb, 0x16, 0x1b, 0x04, 0x6d, 0x9a, 0xf1]);
    // Set vault ATAs at offsets 139/171
    let base_vault = Pubkey::new_from_array([0x11; 32]);
    let quote_vault = Pubkey::new_from_array([0x22; 32]);
    data[139..171].copy_from_slice(&base_vault.to_bytes());
    data[171..203].copy_from_slice(&quote_vault.to_bytes());
    // coin_creator at offset 211
    let cc = Pubkey::new_from_array([0x42; 32]);
    data[211..243].copy_from_slice(&cc.to_bytes());
    // is_mayhem_mode at offset 243
    data[243] = 1;
    // is_cashback_coin at offset 244
    data[244] = 1;

    let meta = parse_pumpswap_pool_meta(&data).unwrap();
    assert_eq!(meta.coin_creator, cc);
    assert!(meta.is_mayhem_mode);
    assert!(meta.is_cashback_coin);
    assert_eq!(meta.pool_base_token_account, base_vault);
    assert_eq!(meta.pool_quote_token_account, quote_vault);
}

#[test]
fn parse_pool_meta_too_short() {
    let data = vec![0u8; 200];
    assert!(parse_pumpswap_pool_meta(&data).is_none());
}

#[test]
fn pda_derivations_are_deterministic() {
    let user = test_pubkey(1);
    let a = pumpswap_user_vol_accumulator(&user);
    let b = pumpswap_user_vol_accumulator(&user);
    assert_eq!(a, b, "PDA must be deterministic");

    let fc1 = pumpswap_fee_config_pda();
    let fc2 = pumpswap_fee_config_pda();
    assert_eq!(fc1, fc2, "fee_config PDA must be deterministic");
}
