-- Persist CPMM and Whirlpool pool metadata so restarts don't require PDA derivation.
-- Mint pairs are sorted (mint_a < mint_b).
CREATE TABLE IF NOT EXISTS cpmm_pools (
    mint_a       TEXT NOT NULL,
    mint_b       TEXT NOT NULL,
    pool_address TEXT NOT NULL,
    config_address TEXT NOT NULL DEFAULT '',
    added_at     TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (mint_a, mint_b)
);

CREATE TABLE IF NOT EXISTS whirlpool_pools (
    mint_a       TEXT NOT NULL,
    mint_b       TEXT NOT NULL,
    pool_address TEXT NOT NULL,
    tick_spacing INTEGER NOT NULL DEFAULT 0,
    added_at     TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (mint_a, mint_b)
);
