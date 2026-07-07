//! Whitelist module — loading, persistence, and updating of arbitrage candidate mints
//!
//! The whitelist is the "index" for the event-driven approach:
//! - On startup, load known mints from SQLite
//! - At runtime, if a new mint is verified to exist on both PumpSwap + DLMM, add to whitelist
//! - Persist to SQLite, survives restarts
//!
//! Note: The whitelist only stores mint addresses, not prices or pool info.
//! Prices are queried in real-time on each swap event trigger, via pool_cache.

use log::debug;
use std::collections::HashSet;

/// Whitelist in-memory state
#[derive(Debug, Clone, Default)]
pub struct Whitelist {
    /// Profitable mints — scanned first after event trigger
    pub profitable: HashSet<String>,
    /// Verified mints — scanned normally after event trigger
    pub verified: HashSet<String>,
    /// All mints (profitable ∪ verified) — for fast dedup lookup
    all: HashSet<String>,
    /// Blacklist — permanently excluded, won't be re-added by discovery
    blacklisted: HashSet<String>,
}

impl Whitelist {
    /// Load whitelist from SQLite
    pub fn load() -> Self {
        let (profitable, verified, blacklisted) = crate::persistence::whitelist_load_all();

        let mut all = HashSet::new();
        for m in &profitable {
            all.insert(m.clone());
        }
        for m in &verified {
            all.insert(m.clone());
        }

        let profitable: HashSet<_> = profitable.into_iter().collect();
        let verified: HashSet<_> = verified.into_iter().collect();
        let blacklisted: HashSet<_> = blacklisted.into_iter().collect();

        log::info!(
            "[WHITELIST] loaded from DB: {} profitable + {} verified + {} blacklisted = {} active",
            profitable.len(),
            verified.len(),
            blacklisted.len(),
            all.len(),
        );

        Whitelist {
            profitable,
            verified,
            all,
            blacklisted,
        }
    }

    /// Check if mint is in the whitelist
    pub fn contains(&self, mint: &str) -> bool {
        self.all.contains(mint)
    }

    /// Check if mint is in the blacklist
    pub fn is_blacklisted(&self, mint: &str) -> bool {
        self.blacklisted.contains(mint)
    }

    /// Add to whitelist after verification passes. Mints in the blacklist are rejected outright.
    pub fn verify_and_add(&mut self, mint: String) -> bool {
        if self.all.contains(&mint) {
            return false;
        }
        if self.blacklisted.contains(&mint) {
            return false;
        }
        let short = mint[..12.min(mint.len())].to_string();
        self.verified.insert(mint.clone());
        self.all.insert(mint.clone());
        crate::persistence::whitelist_upsert(&mint, "verified");
        debug!("[WHITELIST] new verified mint: {}", short);
        true
    }

    /// Mark mint as profitable (move to profitable set)
    pub fn mark_profitable(&mut self, mint: &str) {
        if self.verified.remove(mint) {
            self.profitable.insert(mint.to_string());
            crate::persistence::whitelist_upsert(mint, "profitable");
            debug!(
                "[WHITELIST] mint promoted to profitable: {}",
                &mint[..12.min(mint.len())]
            );
        }
    }

    /// Whitelist size
    pub fn len(&self) -> usize {
        self.all.len()
    }

    /// Persist to SQLite (full overwrite, used for periodic saves)
    pub fn save(&self) {
        // SQLite is already updated incrementally via verify_and_add / mark_profitable / blacklist,
        // save() kept for compatibility but doesn't need full overwrite
    }

    /// Remove single-venue token (pure deletion, does not go into blacklist).
    /// Can be re-added next time discovery detects dual pools.
    pub fn remove_single_venue(&mut self, mint: &str) -> bool {
        if self.blacklisted.contains(mint) {
            return false; // Manually blacklisted tokens are untouched
        }
        let removed = self.verified.remove(mint) || self.profitable.remove(mint);
        if removed {
            self.all.remove(mint);
            crate::persistence::whitelist_delete(mint);
            let short = &mint[..12.min(mint.len())];
            debug!("[WHITELIST] single-venue removed: {short}");
        }
        removed
    }
}
