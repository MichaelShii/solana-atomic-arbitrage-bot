-- 001_init.sql — MEVbot initial schema
-- Run: sqlite3 ~/.local/share/mevbot/mevbot.db < migrations/001_init.sql

PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;

CREATE TABLE IF NOT EXISTS whitelist (
    mint TEXT PRIMARY KEY,
    category TEXT NOT NULL DEFAULT 'verified',
    added_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS lb_pairs (
    mint TEXT NOT NULL,
    lb_pair TEXT NOT NULL,
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (mint, lb_pair)
);

CREATE TABLE IF NOT EXISTS dlmm_metadata (
    key TEXT NOT NULL,
    lb_pair TEXT NOT NULL,
    token_x_mint TEXT NOT NULL,
    token_y_mint TEXT NOT NULL,
    bin_step INTEGER NOT NULL,
    base_factor INTEGER NOT NULL DEFAULT 0,
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (key, lb_pair)
);

CREATE TABLE IF NOT EXISTS trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    signature TEXT NOT NULL UNIQUE,
    token_mint TEXT NOT NULL,
    buy_venue TEXT NOT NULL,
    sell_venue TEXT NOT NULL,
    investment_sol REAL NOT NULL,
    estimated_profit_sol REAL NOT NULL,
    actual_profit_sol REAL,
    succeeded INTEGER,
    confirmed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
