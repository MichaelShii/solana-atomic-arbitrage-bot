use solana_sdk::pubkey::Pubkey;

// ============================================================
// Pool metadata read (is_cashback_coin, coin_creator)
// ============================================================

/// Parsed PumpSwap pool metadata & vault ATAs from on-chain account data.
///
/// Pool account layout (packed repr(C) after 8-byte discriminator):
///   offset 131: pool_base_token_account  (Pubkey, 32 bytes) → raw offset 139
///   offset 163: pool_quote_token_account (Pubkey, 32 bytes) → raw offset 171
///   offset 203: coin_creator             (Pubkey, 32 bytes) → raw offset 211
///   offset 235: is_mayhem_mode           (u8, 1 byte)       → raw offset 243
///   offset 236: is_cashback_coin         (u8, 1 byte)       → raw offset 244
///
/// Vaults are read directly from the pool account — NOT derived via ATA,
/// because the pool may use a non-standard authority (e.g. a PDA).
#[derive(Clone)]
pub struct PumpSwapPoolMeta {
    pub coin_creator: Pubkey,
    pub is_mayhem_mode: bool,
    pub is_cashback_coin: bool,
    pub pool_base_token_account: Pubkey,
    pub pool_quote_token_account: Pubkey,
}

/// Parse pool metadata + vault ATAs from raw account data (includes 8-byte discriminator).
/// Returns None if the account is too short to contain vault addresses.
///
/// Pool struct layout (after 8-byte discriminator), from pAMMBay6oceH IDL:
///   offset   8: pool_bump: u8
///   offset   9: index: u16
///   offset  11: creator: Pubkey
///   offset  43: base_mint: Pubkey
///   offset  75: quote_mint: Pubkey
///   offset 107: lp_mint: Pubkey
///   offset 139: pool_base_token_account: Pubkey   (read from data directly)
///   offset 171: pool_quote_token_account: Pubkey  (read from data directly)
///   ... remaining fields (coin_creator, mayhem, cashback) may not exist
///       in older pool accounts. Default to empty/false when absent.
pub fn parse_pumpswap_pool_meta(data: &[u8]) -> Option<PumpSwapPoolMeta> {
    // Minimum: discriminator(8) + through pool_quote_token_account(171+32=203)
    // This matches read_pumpswap_pool() in pool_cache/bonding_curve/pda.rs.
    if data.len() < 203 {
        return None;
    }
    let pool_base_token_account = Pubkey::new_from_array(data[139..171].try_into().ok()?);
    let pool_quote_token_account = Pubkey::new_from_array(data[171..203].try_into().ok()?);

    // coin_creator, is_mayhem_mode, is_cashback_coin exist only in newer pools.
    // Use defaults when account data is too short (pre-program-upgrade pools).
    let coin_creator = if data.len() >= 243 {
        Pubkey::new_from_array(data[211..243].try_into().ok()?)
    } else {
        Pubkey::default()
    };
    let is_mayhem_mode = data.len() >= 244 && data[243] != 0;
    let is_cashback_coin = data.len() >= 245 && data[244] != 0;

    Some(PumpSwapPoolMeta {
        coin_creator,
        is_mayhem_mode,
        is_cashback_coin,
        pool_base_token_account,
        pool_quote_token_account,
    })
}
