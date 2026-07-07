//! Raydium AMMv4 instruction builder
#![allow(dead_code)] // reserved for venue expansion

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

use super::{AMMV4_SWAP_DISCRIMINATOR, AMMV4_SWAP_OUT_DISCRIMINATOR, SERUM_PROGRAM, TOKEN_PROGRAM};

/// Build AMMv4 SwapBaseIn instruction (coin→pc)
#[allow(clippy::too_many_arguments)]
pub fn build_ammv4_swap_ix(
    wallet: &Pubkey,
    pool_state: &Pubkey,
    open_orders: &Pubkey,
    target_orders: &Pubkey,
    coin_vault: &Pubkey,
    pc_vault: &Pubkey,
    market: &Pubkey,
    bids: &Pubkey,
    asks: &Pubkey,
    event_queue: &Pubkey,
    user_input_ata: &Pubkey,
    user_output_ata: &Pubkey,
    amount_in: u64,
    min_amount_out: u64,
    amm_v4: &Pubkey,
) -> Instruction {
    let token_program = Pubkey::from_str(TOKEN_PROGRAM).unwrap();
    let serum_program = Pubkey::from_str(SERUM_PROGRAM).unwrap();

    let authority =
        Pubkey::find_program_address(&[b"amm authority", &pool_state.to_bytes()], amm_v4).0;

    let accounts = vec![
        AccountMeta::new_readonly(token_program, false),
        AccountMeta::new(*pool_state, false),
        AccountMeta::new_readonly(authority, false),
        AccountMeta::new(*open_orders, false),
        AccountMeta::new(*target_orders, false),
        AccountMeta::new(*coin_vault, false),
        AccountMeta::new(*pc_vault, false),
        AccountMeta::new_readonly(serum_program, false),
        AccountMeta::new(*market, false),
        AccountMeta::new(*bids, false),
        AccountMeta::new(*asks, false),
        AccountMeta::new(*event_queue, false),
        AccountMeta::new(*user_input_ata, false),
        AccountMeta::new(*user_output_ata, false),
        AccountMeta::new(*wallet, true),
    ];

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&AMMV4_SWAP_DISCRIMINATOR);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());

    Instruction {
        program_id: *amm_v4,
        accounts,
        data,
    }
}

/// Build AMMv4 SwapBaseOut instruction (pc→coin, reverse of SwapBaseIn)
#[allow(clippy::too_many_arguments)]
pub fn build_ammv4_swap_out_ix(
    wallet: &Pubkey,
    pool_state: &Pubkey,
    open_orders: &Pubkey,
    target_orders: &Pubkey,
    coin_vault: &Pubkey,
    pc_vault: &Pubkey,
    market: &Pubkey,
    bids: &Pubkey,
    asks: &Pubkey,
    event_queue: &Pubkey,
    user_input_ata: &Pubkey,
    user_output_ata: &Pubkey,
    max_amount_in: u64,
    amount_out: u64,
    amm_v4: &Pubkey,
) -> Instruction {
    let token_program = Pubkey::from_str(TOKEN_PROGRAM).unwrap();
    let serum_program = Pubkey::from_str(SERUM_PROGRAM).unwrap();

    let authority =
        Pubkey::find_program_address(&[b"amm authority", &pool_state.to_bytes()], amm_v4).0;

    let accounts = vec![
        AccountMeta::new_readonly(token_program, false),
        AccountMeta::new(*pool_state, false),
        AccountMeta::new_readonly(authority, false),
        AccountMeta::new(*open_orders, false),
        AccountMeta::new(*target_orders, false),
        AccountMeta::new(*coin_vault, false),
        AccountMeta::new(*pc_vault, false),
        AccountMeta::new_readonly(serum_program, false),
        AccountMeta::new(*market, false),
        AccountMeta::new(*bids, false),
        AccountMeta::new(*asks, false),
        AccountMeta::new(*event_queue, false),
        AccountMeta::new(*user_input_ata, false),
        AccountMeta::new(*user_output_ata, false),
        AccountMeta::new(*wallet, true),
    ];

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&AMMV4_SWAP_OUT_DISCRIMINATOR);
    data.extend_from_slice(&max_amount_in.to_le_bytes());
    data.extend_from_slice(&amount_out.to_le_bytes());

    Instruction {
        program_id: *amm_v4,
        accounts,
        data,
    }
}

/// Read bids, asks, event_queue addresses from a Serum/OpenBook market account
pub async fn fetch_serum_market_addrs(
    rpc: &RpcClient,
    market_addr: &str,
) -> Option<(Pubkey, Pubkey, Pubkey)> {
    let market_pk = Pubkey::from_str(market_addr).ok()?;
    let account = rpc.get_account(&market_pk).await.ok()?;
    let data = &account.data;

    if data.len() < 344 {
        log::warn!("Market account too short: {} bytes", data.len());
        return None;
    }

    let read_pk = |off: usize| -> Option<Pubkey> {
        let bytes: [u8; 32] = data[off..off + 32].try_into().ok()?;
        Some(Pubkey::new_from_array(bytes))
    };

    Some((
        read_pk(280)?, // bids
        read_pk(312)?, // asks
        read_pk(248)?, // event_queue
    ))
}

/// Read AMMv4 vault token account balances (raw u64)
pub async fn read_ammv4_vault_amounts(
    rpc: &RpcClient,
    vault_a: &str,
    vault_b: &str,
) -> Option<(u64, u64)> {
    let va = Pubkey::from_str(vault_a).ok()?;
    let vb = Pubkey::from_str(vault_b).ok()?;
    let accounts = rpc.get_multiple_accounts(&[va, vb]).await.ok()?;

    let parse = |data: &[u8]| -> Option<u64> {
        if data.len() < 72 {
            return None;
        }
        Some(u64::from_le_bytes(data[64..72].try_into().ok()?))
    };

    let a = parse(accounts.first()?.as_ref()?.data.as_slice())?;
    let b = parse(accounts.get(1)?.as_ref()?.data.as_slice())?;
    Some((a, b))
}
