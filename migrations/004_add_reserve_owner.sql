-- 004: DLMM reserve account owner (token program) cache
-- Survives restarts so TokenProgram detection works immediately.

CREATE TABLE IF NOT EXISTS reserve_token_programs (
    reserve_address TEXT PRIMARY KEY,
    owner_program   TEXT NOT NULL,
    added_at        TEXT NOT NULL DEFAULT (datetime('now'))
);
