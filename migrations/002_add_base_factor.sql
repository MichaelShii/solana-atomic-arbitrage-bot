-- 002_add_base_factor.sql — M-03: store base_factor per pool for fee tier detection
-- Fee bps = (bin_step * 10000) / base_factor
-- Existing rows default to 0 (interpreted as "unknown" → fallback to 25 bps)

ALTER TABLE dlmm_metadata ADD COLUMN base_factor INTEGER NOT NULL DEFAULT 0;
