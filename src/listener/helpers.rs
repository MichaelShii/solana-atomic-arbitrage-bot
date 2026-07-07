//! Shared helper functions for the listener module.

use std::collections::HashSet;

/// Mask the API key in the URL
pub(crate) fn mask_url(url: &str) -> String {
    if let Some(pos) = url.find('?') {
        format!("{}?...", &url[..pos])
    } else if url.len() > 50 {
        format!("{}...", &url[..50])
    } else {
        url.to_string()
    }
}

/// Extract all Solana pubkey candidates from log strings (base58-encoded).
///
/// Strategy: scan line by line, take tokens of length 32-44 containing only base58 characters,
/// filter out known non-mint addresses (program IDs / native mints / fee recipients).
/// The returned candidate set is used for whitelist matching — only successful matches enter the channel.
pub(crate) fn extract_mint_candidates(logs: &[String]) -> HashSet<String> {
    let mut candidates = HashSet::with_capacity(8);
    for log in logs {
        for token in log.split(|c: char| !c.is_alphanumeric()) {
            if token.len() >= 32
                && token.len() <= 44
                && is_base58(token)
                && !KNOWN_FILTER.contains(&token)
            {
                candidates.insert(token.to_string());
            }
        }
    }
    candidates
}

pub(crate) fn is_base58(s: &str) -> bool {
    s.bytes().all(|b| {
        matches!(
            b,
            b'1'..=b'9'
                | b'A'..=b'H'
                | b'J'..=b'N'
                | b'P'..=b'Z'
                | b'a'..=b'k'
                | b'm'..=b'z'
        )
    })
}

/// Known non-meme mint addresses — excluded when extracting candidates
static KNOWN_FILTER: &[&str] = &[
    "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",
    "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
    "So11111111111111111111111111111111111111112",
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB",
    "CebN5WGQ4jvEPvsVU4EoHEpgT1mKQ7AFUbxmAhvFUWrQ",
    "11111111111111111111111111111111",
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
    "ComputeBudget111111111111111111111111111111",
    "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
];

/// Determine which program the transaction comes from
pub(crate) fn determine_program(logs: &[String]) -> String {
    for log in logs {
        if log.contains("pAMMBay6oceH") {
            return "pumpfun".to_string();
        }
        if log.contains("LBUZKhRx") {
            return "dlmm".to_string();
        }
        if log.contains("CPMMoo8L3F4") {
            return "cpmm".to_string();
        }
        if log.contains("whirLbMiicVd") {
            return "whirlpool".to_string();
        }
    }
    "unknown".to_string()
}
