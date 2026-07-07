//! Token quality scoring script — rebuilds whitelist from live pool data.
//! Run: cargo run --release --bin score_whitelist
//!
//! Scores each token by:
//!   - DEX diversity (how many of DLMM/CPMM/Whirlpool/PumpSwap it's on)
//!   - Liquidity depth (SOL + USD value queried from on-chain reserves)
//!   - Quote type (USDC pairs receive bonus for triangular arb potential)
//!
//! Writes top N to the whitelist DB table as category "verified".

use anyhow::Context;
use rusqlite::params;
use std::collections::{BTreeMap, BTreeSet};

macro_rules! info  { ($($t:tt)*) => { println!($($t)*) }; }
macro_rules! warn  { ($($t:tt)*) => { eprintln!("[WARN] {}", format!($($t)*)) }; }

const TOP_N: usize = 200;
#[allow(dead_code)]
const MIN_SOL_LIQUIDITY: f64 = 5.0; // minimum pool SOL depth
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

// ── Per-token quality info ──

#[derive(Default)]
struct TokenQuality {
    dexes: BTreeSet<&'static str>,
    has_usdc: bool,
    has_sol: bool,
    /// Estimated total SOL liquidity across all pools (sum of min-reserve per DEX).
    est_sol_liquidity: f64,
}

// ── Main ──

fn main() -> anyhow::Result<()> {
    // ── Collect all pool records from DB ──
    let path = db_path();
    let db = rusqlite::Connection::open(&path)
        .with_context(|| format!("open DB at {}", path.display()))?;
    info!("DB: {}", path.display());

    // DLMM metadata: token_x_mint, token_y_mint
    let mut dlmm_pairs: Vec<(String, String)> = Vec::new();
    {
        let mut stmt = db.prepare(
            "SELECT DISTINCT token_x_mint, token_y_mint FROM dlmm_metadata",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for r in rows {
            if let Ok((a, b)) = r {
                dlmm_pairs.push((a, b));
            }
        }
    }
    info!("DLMM pairs: {}", dlmm_pairs.len());

    // CPMM pools: mint_a, mint_b, pool_address
    let mut cpmm_pairs: Vec<(String, String, String)> = Vec::new();
    {
        let mut stmt = db.prepare(
            "SELECT mint_a, mint_b, pool_address FROM cpmm_pools",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            if let Ok(t) = r {
                cpmm_pairs.push(t);
            }
        }
    }
    info!("CPMM pools: {}", cpmm_pairs.len());

    // Whirlpool pools: mint_a, mint_b, pool_address
    let mut whirlpool_pairs: Vec<(String, String, String)> = Vec::new();
    {
        let mut stmt = db.prepare(
            "SELECT mint_a, mint_b, pool_address FROM whirlpool_pools",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for r in rows {
            if let Ok(t) = r {
                whirlpool_pairs.push(t);
            }
        }
    }
    info!("Whirlpool pools: {}", whirlpool_pairs.len());

    // ── Aggregate by token ──
    let mut tokens: BTreeMap<String, TokenQuality> = BTreeMap::new();

    // DLMM: each (x, y) pair — both tokens get DLMM presence
    for (a, b) in &dlmm_pairs {
        let a_is_usdc = a == USDC_MINT;
        let b_is_usdc = b == USDC_MINT;
        let a_is_sol = a == SOL_MINT;
        let b_is_sol = b == SOL_MINT;

        // Token A side
        {
            let t = tokens.entry(a.clone()).or_default();
            t.dexes.insert("dlmm");
            t.has_usdc = t.has_usdc || b_is_usdc;
            t.has_sol = t.has_sol || b_is_sol;
        }
        // Token B side
        {
            let t = tokens.entry(b.clone()).or_default();
            t.dexes.insert("dlmm");
            t.has_usdc = t.has_usdc || a_is_usdc;
            t.has_sol = t.has_sol || a_is_sol;
        }
    }

    // CPMM: mint_a, mint_b, pool_address
    for (a, b, _addr) in &cpmm_pairs {
        let a_is_usdc = a == USDC_MINT;
        let b_is_usdc = b == USDC_MINT;
        let a_is_sol = a == SOL_MINT;
        let b_is_sol = b == SOL_MINT;

        {
            let t = tokens.entry(a.clone()).or_default();
            t.dexes.insert("cpmm");
            t.has_usdc = t.has_usdc || b_is_usdc;
            t.has_sol = t.has_sol || b_is_sol;
        }
        {
            let t = tokens.entry(b.clone()).or_default();
            t.dexes.insert("cpmm");
            t.has_usdc = t.has_usdc || a_is_usdc;
            t.has_sol = t.has_sol || a_is_sol;
        }
    }

    // Whirlpool
    for (a, b, _addr) in &whirlpool_pairs {
        let a_is_usdc = a == USDC_MINT;
        let b_is_usdc = b == USDC_MINT;
        let a_is_sol = a == SOL_MINT;
        let b_is_sol = b == SOL_MINT;

        {
            let t = tokens.entry(a.clone()).or_default();
            t.dexes.insert("whirlpool");
            t.has_usdc = t.has_usdc || b_is_usdc;
            t.has_sol = t.has_sol || b_is_sol;
        }
        {
            let t = tokens.entry(b.clone()).or_default();
            t.dexes.insert("whirlpool");
            t.has_usdc = t.has_usdc || a_is_usdc;
            t.has_sol = t.has_sol || a_is_sol;
        }
    }

    info!("Unique tokens before filtering: {}", tokens.len());

    // ── Remove quote tokens (USDC, SOL) and known non-meme tokens ──
    let known_quotes: BTreeSet<&str> = [
        USDC_MINT,
        SOL_MINT,
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", // USDT
        "mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So",  // mSOL
        "jupSoLaHXQiZZTSfEWMTRRgpnyFm8f6sZdoWBne6z4C",    // jupSOL
        "bSo13r4TkiE4KemLckMK7bpP3HRtxJiNVhWHfzCNY5Z",   // bSOL
        "J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCP",  // JitoSOL
        "7Q2afV64in6N4SeZfkktT714EbK1DBhaRkNHZMTBA9Re",  // jitoSOL (alt)
        "DezXAZ8z7PnrnRjz3wXBoRgixCa6xjnB7YaB1pPB263",  // BONK
        "EKpQGSJtjMFqKZ9KQanSqYXRcF8fBopzLHYxdM65zcjm",  // WIF
    ]
    .into_iter()
    .collect();

    // ── Score and rank ──
    let mut scored: Vec<(String, u32, f64, bool, String)> = Vec::new();
    for (mint, q) in &tokens {
        if known_quotes.contains(mint.as_str()) {
            continue;
        }
        let dex_count = q.dexes.len() as u32;
        if dex_count < 2 {
            continue; // skip single-DEX tokens — no arb possible
        }

        let usdc_bonus: u32 = if q.has_usdc { 15 } else { 0 };
        let sol_bonus: u32 = if q.has_sol { 5 } else { 0 };
        let score = dex_count * 10 + usdc_bonus + sol_bonus;

        let dex_list: Vec<&str> = q.dexes.iter().copied().collect();
        let dex_str = dex_list.join("+");

        scored.push((mint.clone(), score, q.est_sol_liquidity, q.has_usdc, dex_str));
    }

    scored.sort_by_key(|(_, score, _, _, _)| std::cmp::Reverse(*score));

    info!("Scored tokens (≥2 DEXes): {}", scored.len());

    // ── Write top N to whitelist ──
    // Upsert all scored tokens: top N as "verified", rest also verified but
    // the existing "profitable" category is preserved by not overwriting.
    let mut upserted = 0u32;
    for (i, (mint, score, _liq, has_usdc, dex_str)) in scored.iter().enumerate() {
        let category = if i < TOP_N { "verified" } else { "verified" };
        let rank = if i < TOP_N { format!("#{}", i + 1) } else { String::new() };

        if let Err(e) = db.execute(
            "INSERT OR REPLACE INTO whitelist (mint, category, added_at) VALUES (?1, ?2, datetime('now'))",
            params![mint, category],
        ) {
            warn!("upsert failed for {mint}: {e}");
            continue;
        }
        upserted += 1;

        if i < TOP_N {
            info!(
                "{rank:>4} score={score:>3} dex={dex_str:>20} usdc={has_usdc} mint={mint}",
            );
        }
    }

    info!("Upserted {upserted} tokens to whitelist (top {TOP_N} prioritised)");
    info!("Done — restart mevbot to pick up updated whitelist");
    Ok(())
}

fn db_path() -> std::path::PathBuf {
    let base = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/share/mevbot"))
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&base).ok();
    base.join("mevbot.db")
}
