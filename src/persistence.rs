//! Unified SQLite persistence — replaces old JSON files
//!
//! Database location: ~/.local/share/mevbot/mevbot.db
//! Schema defined in migrations/001_init.sql (single source of truth, compile-time include_str! embedding).
//! Run migrate_json.py before first deployment to import historical JSON data.
//!
//! WAL mode, supports concurrent reads/writes. Single Mutex guards all write operations.

use log::{info, warn};
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

static DB: LazyLock<Mutex<Connection>> = LazyLock::new(|| {
    let conn = open_or_create_db();
    Mutex::new(conn)
});

// ============================================================
// Initialization
// ============================================================

/// Ensure database is initialized (idempotent, called in main.rs)
pub fn init_db() {
    drop(DB.lock().expect("db lock"));
    info!("[DB] initialized at {}", db_path().display());
}

fn db_path() -> PathBuf {
    let base = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/mevbot"))
        .unwrap_or_else(|_| PathBuf::from("."));
    std::fs::create_dir_all(&base).ok();
    base.join("mevbot.db")
}

fn open_or_create_db() -> Connection {
    let path = db_path();
    info!("[DB] opening {}", path.display());

    let conn = Connection::open(&path).expect("failed to open SQLite database");

    // Schema defined in migrations/ (single source of truth)
    conn.execute_batch(include_str!("../migrations/001_init.sql"))
        .expect("failed to run 001_init.sql");
    // 002 is safe to re-run: ALTER TABLE ... ADD COLUMN with IF NOT EXISTS via ignore
    let _ = conn.execute_batch(include_str!("../migrations/002_add_base_factor.sql"));
    let _ = conn.execute_batch(include_str!("../migrations/003_add_bitmap_extension.sql"));
    let _ = conn.execute_batch(include_str!("../migrations/004_add_reserve_owner.sql"));
    let _ = conn.execute_batch(include_str!("../migrations/005_add_cpmm_whirlpool_pools.sql"));

    conn
}

// ============================================================
// Public API — Whitelist
// ============================================================

/// Load whitelist: category → mint[]
pub fn whitelist_load_all() -> (Vec<String>, Vec<String>, Vec<String>) {
    let db = DB.lock().expect("db lock");
    let mut profitable = Vec::new();
    let mut verified = Vec::new();
    let mut blacklisted = Vec::new();

    let mut stmt = db
        .prepare("SELECT mint, category FROM whitelist")
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query");

    for (mint, cat) in rows.flatten() {
        match cat.as_str() {
            "profitable" => profitable.push(mint),
            "blacklisted" => blacklisted.push(mint),
            _ => verified.push(mint),
        }
    }
    (profitable, verified, blacklisted)
}

/// Upsert a mint into the whitelist
pub fn whitelist_upsert(mint: &str, category: &str) {
    let db = DB.lock().expect("db lock");
    if let Err(e) = db.execute(
        "INSERT OR REPLACE INTO whitelist (mint, category, added_at) VALUES (?1, ?2, datetime('now'))",
        params![mint, category],
    ) {
        warn!("[DB] whitelist_upsert failed: {e}");
    }
}

/// Directly delete mint from whitelist (not blacklist, can be re-discovered by discovery)
#[allow(dead_code)]
pub fn whitelist_delete(mint: &str) {
    let db = DB.lock().expect("db lock");
    if let Err(e) = db.execute(
        "DELETE FROM whitelist WHERE mint=?1 AND category!='blacklisted'",
        params![mint],
    ) {
        warn!("[DB] whitelist_delete failed: {e}");
    }
}

/// Extract all whitelist mints (profitable ∪ verified)
#[allow(dead_code)]
pub fn whitelist_get_all_active() -> Vec<String> {
    let db = DB.lock().expect("db lock");
    let mut stmt = db
        .prepare("SELECT mint FROM whitelist WHERE category != 'blacklisted'")
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query");
    rows.filter_map(|r| r.ok()).collect()
}

// ============================================================
// Public API — LB Pairs
// ============================================================

/// Load all lb_pair mappings into memory
pub fn lb_pairs_load_all() -> std::collections::HashMap<String, String> {
    let db = DB.lock().expect("db lock");
    let mut stmt = db
        .prepare("SELECT mint, lb_pair FROM lb_pairs")
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query");
    rows.filter_map(|r| r.ok()).collect()
}

/// Insert new lb_pair mapping (mint_a and mint_b both point to the same lb_pair)
pub fn lb_pairs_insert_both(mint_a: &str, mint_b: &str, lb_pair: &str) {
    let db = DB.lock().expect("db lock");
    let tx = match db.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => {
            warn!("[DB] lb_pairs transaction failed: {e}");
            return;
        }
    };
    for mint in [mint_a, mint_b] {
        if let Err(e) = tx.execute(
            "INSERT OR REPLACE INTO lb_pairs (mint, lb_pair, added_at) VALUES (?1, ?2, datetime('now'))",
            params![mint, lb_pair],
        ) {
            warn!("[DB] lb_pairs_insert failed for {mint}: {e}");
        }
    }
    if let Err(e) = tx.commit() {
        warn!("[DB] lb_pairs commit failed: {e}");
    }
}

/// Full overwrite of lb_pairs (for rebuilding after eviction)
pub fn lb_pairs_replace_all(entries: &[(String, String)]) {
    let db = DB.lock().expect("db lock");
    let tx = match db.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => {
            warn!("[DB] lb_pairs_replace transaction failed: {e}");
            return;
        }
    };
    if let Err(e) = tx.execute("DELETE FROM lb_pairs", []) {
        warn!("[DB] lb_pairs delete failed: {e}");
    }
    for (mint, lb_pair) in entries {
        if let Err(e) = tx.execute(
            "INSERT INTO lb_pairs (mint, lb_pair) VALUES (?1, ?2)",
            params![mint, lb_pair],
        ) {
            warn!("[DB] lb_pairs insert failed for {mint}: {e}");
        }
    }
    if let Err(e) = tx.commit() {
        warn!("[DB] lb_pairs_replace commit failed: {e}");
    }
}

// ============================================================
// Public API — DLMM Metadata
// ============================================================

/// DLMM metadata batch write type
/// Tuple: (key, vec of (lb_pair, token_x, token_y, bin_step, base_factor, bitmap_extension))
pub type DlmmMetaEntries = [(
    String,
    Vec<(String, String, String, u16, u16, Option<String>)>,
)];

/// DLMM metadata record (for loading from SQLite)
#[derive(Debug, Clone)]
pub struct DlmmMetaRow {
    pub key: String,
    pub lb_pair: String,
    pub token_x_mint: String,
    pub token_y_mint: String,
    pub bin_step: u16,
    pub base_factor: u16,
    pub bin_array_bitmap_extension: Option<String>,
}

/// Load all DLMM metadata
pub fn dlmm_metadata_load_all() -> Vec<DlmmMetaRow> {
    let db = DB.lock().expect("db lock");
    let mut stmt = db
        .prepare("SELECT key, lb_pair, token_x_mint, token_y_mint, bin_step, base_factor, bin_array_bitmap_extension FROM dlmm_metadata")
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| {
            Ok(DlmmMetaRow {
                key: row.get(0)?,
                lb_pair: row.get(1)?,
                token_x_mint: row.get(2)?,
                token_y_mint: row.get(3)?,
                bin_step: row.get::<_, i64>(4)? as u16,
                base_factor: row.get::<_, i64>(5)? as u16,
                bin_array_bitmap_extension: row.get(6)?,
            })
        })
        .expect("query");
    rows.filter_map(|r| r.ok()).collect()
}

/// Overwrite DLMM metadata (full replace)
pub fn dlmm_metadata_replace_all(entries: &DlmmMetaEntries) {
    let db = DB.lock().expect("db lock");
    let tx = match db.unchecked_transaction() {
        Ok(t) => t,
        Err(e) => {
            warn!("[DB] dlmm_metadata transaction failed: {e}");
            return;
        }
    };
    if let Err(e) = tx.execute("DELETE FROM dlmm_metadata", []) {
        warn!("[DB] dlmm_metadata delete failed: {e}");
    }
    for (key, pools) in entries {
        for (lb_pair, token_x, token_y, bin_step, base_factor, bitmap_ext) in pools {
            if let Err(e) = tx.execute(
                "INSERT INTO dlmm_metadata (key, lb_pair, token_x_mint, token_y_mint, bin_step, base_factor, bin_array_bitmap_extension) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![key, lb_pair, token_x, token_y, *bin_step as i64, *base_factor as i64, bitmap_ext],
            ) {
                warn!("[DB] dlmm_metadata insert failed: {e}");
            }
        }
    }
    if let Err(e) = tx.commit() {
        warn!("[DB] dlmm_metadata commit failed: {e}");
    }
}

// ============================================================
// Public API — Trades
// ============================================================

/// Write on submission (confirmation status unknown)
pub fn trade_insert_submitted(
    sig: &str,
    mint: &str,
    buy_venue: &str,
    sell_venue: &str,
    investment_sol: f64,
    estimated_profit_sol: f64,
) {
    let db = DB.lock().expect("db lock");
    if let Err(e) = db.execute(
        "INSERT OR IGNORE INTO trades (signature, token_mint, buy_venue, sell_venue, investment_sol, estimated_profit_sol)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![sig, mint, buy_venue, sell_venue, investment_sol, estimated_profit_sol],
    ) {
        warn!("[DB] trade_insert_submitted failed: {e}");
    }
}

/// Update actual PnL after on-chain confirmation
pub fn trade_update_confirmed(sig: &str, actual_pnl_sol: f64, succeeded: bool) {
    let db = DB.lock().expect("db lock");
    if let Err(e) = db.execute(
        "UPDATE trades SET actual_profit_sol=?1, succeeded=?2, confirmed_at=datetime('now') WHERE signature=?3",
        params![actual_pnl_sol, succeeded as i64, sig],
    ) {
        warn!("[DB] trade_update_confirmed failed: {e}");
    }
}

// ============================================================
// Public API — Reserve Token Programs (survives restarts)
// ============================================================

/// Load all known reserve→owner mappings into a HashMap.
/// Called at startup to warm the token program cache.
pub fn reserve_owners_load_all() -> std::collections::HashMap<String, String> {
    let db = DB.lock().expect("db lock");
    let mut stmt = db
        .prepare("SELECT reserve_address, owner_program FROM reserve_token_programs")
        .expect("prepare");
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query");
    rows.filter_map(|r| r.ok()).collect()
}

/// Save a reserve→owner mapping (upsert).
pub fn reserve_owner_save(reserve_address: &str, owner_program: &str) {
    let db = DB.lock().expect("db lock");
    if let Err(e) = db.execute(
        "INSERT OR REPLACE INTO reserve_token_programs (reserve_address, owner_program, added_at) VALUES (?1, ?2, datetime('now'))",
        params![reserve_address, owner_program],
    ) {
        log::warn!("[DB] reserve_owner_save failed: {e}");
    }
}

// ============================================================
// CPMM / Whirlpool pool metadata persistence
// ============================================================

/// Load all persisted CPMM pool addresses.
pub fn cpmm_pools_load_all() -> Vec<(String, String, String, String)> {
    let db = DB.lock().expect("db lock");
    let mut stmt = db
        .prepare("SELECT mint_a, mint_b, pool_address, config_address FROM cpmm_pools")
        .expect("cpmm_pools select");
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .expect("cpmm_pools query")
    .filter_map(|r| r.ok())
    .collect()
}

/// Persist a discovered CPMM pool.
pub fn cpmm_pool_save(mint_a: &str, mint_b: &str, pool: &str, config: &str) {
    if let Ok(db) = DB.lock() {
        let _ = db.execute(
            "INSERT OR REPLACE INTO cpmm_pools (mint_a, mint_b, pool_address, config_address, added_at) VALUES (?1,?2,?3,?4,datetime('now'))",
            params![mint_a, mint_b, pool, config],
        );
    }
}

/// Load all persisted Whirlpool pool addresses.
pub fn whirlpool_pools_load_all() -> Vec<(String, String, String, i32)> {
    let db = DB.lock().expect("db lock");
    let mut stmt = db
        .prepare("SELECT mint_a, mint_b, pool_address, tick_spacing FROM whirlpool_pools")
        .expect("whirlpool_pools select");
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .expect("whirlpool_pools query")
    .filter_map(|r| r.ok())
    .collect()
}

/// Persist a discovered Whirlpool pool.
pub fn whirlpool_pool_save(mint_a: &str, mint_b: &str, pool: &str, tick_spacing: u16) {
    if let Ok(db) = DB.lock() {
        let _ = db.execute(
            "INSERT OR REPLACE INTO whirlpool_pools (mint_a, mint_b, pool_address, tick_spacing, added_at) VALUES (?1,?2,?3,?4,datetime('now'))",
            params![mint_a, mint_b, pool, tick_spacing as i32],
        );
    }
}

/// Query today's trade summary
#[allow(dead_code)]
pub fn trades_daily_summary(date: &str) -> (i64, f64, i64, f64) {
    let db = DB.lock().expect("db lock");
    let (total, pnl): (i64, f64) = db
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(actual_profit_sol), 0) FROM trades WHERE date(created_at)=?1 AND succeeded=1",
            params![date],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, 0.0));
    let (failed, wasted): (i64, f64) = db
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(ABS(actual_profit_sol)), 0) FROM trades WHERE date(created_at)=?1 AND succeeded=0",
            params![date],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, 0.0));
    (total, pnl, failed, wasted)
}
