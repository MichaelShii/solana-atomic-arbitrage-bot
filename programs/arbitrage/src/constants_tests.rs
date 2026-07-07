use super::*;

/// sha256("global:route_pump_to_dlmm")[..8] frozen.
#[test]
fn route_pump_to_dlmm_disc_known() {
    assert_eq!(
        ROUTE_PUMP_TO_DLMM_DISC,
        [0x8b, 0xe8, 0x20, 0x55, 0xc1, 0xb0, 0xc1, 0xe9]
    );
}

/// sha256("global:route_dlmm_to_pump")[..8] frozen.
#[test]
fn route_dlmm_to_pump_disc_known() {
    assert_eq!(
        ROUTE_DLMM_TO_PUMP_DISC,
        [0x17, 0x6e, 0xcc, 0x5d, 0xdd, 0x93, 0x51, 0x95]
    );
}

#[test]
fn pump_buy_disc_matches_idl() {
    assert_eq!(
        PUMP_BUY_DISC,
        [0xc6, 0x2e, 0x15, 0x52, 0xb4, 0xd9, 0xe8, 0x70]
    );
}

#[test]
fn pump_sell_disc_matches_idl() {
    assert_eq!(
        PUMP_SELL_DISC,
        [0x33, 0xe6, 0x85, 0xa4, 0x01, 0x7f, 0x83, 0xad]
    );
}

#[test]
fn dlmm_swap2_disc_known() {
    assert_eq!(
        DLMM_SWAP2_DISC,
        [0x41, 0x4b, 0x3f, 0x4c, 0xeb, 0x5b, 0x5b, 0x88]
    );
}

#[test]
fn ix_data_layout_size() {
    assert_eq!(IX_DATA_LEN, 36);
    assert!(OFF_AMOUNT_IN + 8 <= OFF_MIN_PROFIT);
    assert!(OFF_MIN_PROFIT + 8 <= OFF_MIN_INTERMEDIATE);
    assert!(OFF_MIN_INTERMEDIATE + 8 <= OFF_TRACK_VOLUME);
}

#[test]
fn pump_buy_fixed_len() {
    assert_eq!(PUMP_BUY_FIXED_LEN, 23);
}

#[test]
fn pump_sell_fixed_len() {
    assert_eq!(PUMP_SELL_FIXED_LEN, 21);
}

#[test]
fn dlmm_fixed_len() {
    assert_eq!(DLMM_FIXED_LEN, 9);
}

/// PumpSwap event authority PDA: ["__event_authority"] under PUMP_SWAP_ID.
#[test]
fn pumpswap_event_auth_pda() {
    let (ea, bump) = Pubkey::find_program_address(&[PUMP_EVENT_AUTH_SEED], &PUMP_SWAP_ID);
    assert_eq!(
        ea,
        solana_program::pubkey!("GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR")
    );
    assert!(bump != 0);
}

/// PumpSwap global config PDA: ["global_config"] under PUMP_SWAP_ID.
#[test]
fn pumpswap_global_config_pda() {
    let (gc, bump) = Pubkey::find_program_address(&[PUMP_GLOBAL_CONFIG_SEED], &PUMP_SWAP_ID);
    assert_eq!(
        gc,
        solana_program::pubkey!("ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw")
    );
    assert!(bump != 0);
}

/// PumpSwap fee config PDA: ["fee_config", PUMP_SWAP_ID] under FEE_PROGRAM_ID.
#[test]
fn pumpswap_fee_config_pda() {
    let (fc, bump) = Pubkey::find_program_address(
        &[PUMP_FEE_CONFIG_SEED, PUMP_SWAP_ID.as_ref()],
        &FEE_PROGRAM_ID,
    );
    assert!(!fc.to_bytes().iter().all(|b| *b == 0));
    assert!(bump != 0);
}

/// DLMM oracle PDA: ["oracle", lb_pair] under DLMM_ID.
#[test]
fn dlmm_oracle_pda() {
    let known_lb_pair = solana_program::pubkey!("2vjQ1iSxQmnNzFyvGmSQFxS4orWMsGDTmkPukxE6vJiG");
    let (oracle, bump) =
        Pubkey::find_program_address(&[DLMM_ORACLE_SEED, known_lb_pair.as_ref()], &DLMM_ID);
    assert!(!oracle.to_bytes().iter().all(|b| *b == 0));
    assert!(bump != 0);
}

/// DLMM event authority is a hard-coded constant (not a PDA).
#[test]
fn dlmm_event_auth_known() {
    assert_eq!(
        DLMM_EVENT_AUTH,
        solana_program::pubkey!("D1ZN9Wj1fRSUQfCjhvnu1hqDMT7hzjzBBpi12nVniYD6")
    );
}

/// Borsh layout: given a valid 36-byte instruction data buffer,
/// parse every field and assert correct values.
#[test]
fn borsh_layout_parse() {
    let mut buf = [0u8; IX_DATA_LEN];
    buf[0..8].copy_from_slice(&ROUTE_PUMP_TO_DLMM_DISC);
    buf[OFF_AMOUNT_IN..OFF_AMOUNT_IN + 8].copy_from_slice(&500_000_000u64.to_le_bytes());
    buf[OFF_MIN_PROFIT..OFF_MIN_PROFIT + 8].copy_from_slice(&10_000u64.to_le_bytes());
    buf[OFF_MIN_INTERMEDIATE..OFF_MIN_INTERMEDIATE + 8]
        .copy_from_slice(&42_000_000_000u64.to_le_bytes());
    buf[OFF_TRACK_VOLUME] = 1;
    buf[OFF_DLMM_SOL_IS_X] = 0;
    buf[OFF_PUMP_REMAINING] = 3;
    buf[OFF_DLMM_BIN_ARRAY_COUNT] = 2;

    let disc: [u8; 8] = buf[0..8].try_into().unwrap();
    assert_eq!(disc, ROUTE_PUMP_TO_DLMM_DISC);

    let amount_in = u64::from_le_bytes(buf[OFF_AMOUNT_IN..OFF_AMOUNT_IN + 8].try_into().unwrap());
    assert_eq!(amount_in, 500_000_000);

    let min_profit =
        u64::from_le_bytes(buf[OFF_MIN_PROFIT..OFF_MIN_PROFIT + 8].try_into().unwrap());
    assert_eq!(min_profit, 10_000);

    let min_intermediate = u64::from_le_bytes(
        buf[OFF_MIN_INTERMEDIATE..OFF_MIN_INTERMEDIATE + 8]
            .try_into()
            .unwrap(),
    );
    assert_eq!(min_intermediate, 42_000_000_000);

    let track_volume = buf[OFF_TRACK_VOLUME] != 0;
    assert!(track_volume);

    let dlmm_sol_is_x = buf[OFF_DLMM_SOL_IS_X] != 0;
    assert!(!dlmm_sol_is_x);

    let pump_remaining_count = buf[OFF_PUMP_REMAINING] as usize;
    assert_eq!(pump_remaining_count, 3);

    let dlmm_bin_array_count = buf[OFF_DLMM_BIN_ARRAY_COUNT] as usize;
    assert_eq!(dlmm_bin_array_count, 2);
}

/// Borsh layout: edge values — all zeros for optional fields.
#[test]
fn borsh_layout_edge_zeros() {
    let mut buf = [0u8; IX_DATA_LEN];
    buf[0..8].copy_from_slice(&ROUTE_DLMM_TO_PUMP_DISC);
    buf[OFF_AMOUNT_IN..OFF_AMOUNT_IN + 8].copy_from_slice(&1u64.to_le_bytes());
    buf[OFF_MIN_PROFIT..OFF_MIN_PROFIT + 8].copy_from_slice(&1u64.to_le_bytes());
    buf[OFF_DLMM_BIN_ARRAY_COUNT] = 1;

    let pump_rem = buf[OFF_PUMP_REMAINING] as usize;
    assert_eq!(pump_rem, 0);
    let bin_cnt = buf[OFF_DLMM_BIN_ARRAY_COUNT] as usize;
    assert_eq!(bin_cnt, 1);
    let track_vol = buf[OFF_TRACK_VOLUME] != 0;
    assert!(!track_vol);
}

/// Borsh layout: max values — pump_remaining = 5, bin_count = 4.
#[test]
fn borsh_layout_edge_max() {
    let mut buf = [0u8; IX_DATA_LEN];
    buf[0..8].copy_from_slice(&ROUTE_PUMP_TO_DLMM_DISC);
    buf[OFF_AMOUNT_IN..OFF_AMOUNT_IN + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    buf[OFF_MIN_PROFIT..OFF_MIN_PROFIT + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    buf[OFF_MIN_INTERMEDIATE..OFF_MIN_INTERMEDIATE + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    buf[OFF_TRACK_VOLUME] = 1;
    buf[OFF_DLMM_SOL_IS_X] = 1;
    buf[OFF_PUMP_REMAINING] = 5;
    buf[OFF_DLMM_BIN_ARRAY_COUNT] = 4;

    let amount_in = u64::from_le_bytes(buf[OFF_AMOUNT_IN..OFF_AMOUNT_IN + 8].try_into().unwrap());
    assert_eq!(amount_in, u64::MAX);
    assert!(buf[OFF_TRACK_VOLUME] != 0);
    assert!(buf[OFF_DLMM_SOL_IS_X] != 0);
    assert_eq!(buf[OFF_PUMP_REMAINING] as usize, 5);
    assert_eq!(buf[OFF_DLMM_BIN_ARRAY_COUNT] as usize, 4);
}
