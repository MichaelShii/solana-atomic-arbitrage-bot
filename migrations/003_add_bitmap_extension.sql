-- 003_add_bitmap_extension.sql — R2-H02: store bin_array_bitmap_extension per pool
-- Optional account for DLMM Swap2: tracks bin arrays beyond the base bitmap capacity.
-- NULL = no extension (most pools); non-NULL = extension pubkey.
-- Borsh encoding: offset 248 of lb_pair account, 1-byte tag + optional 32-byte pubkey.

ALTER TABLE dlmm_metadata ADD COLUMN bin_array_bitmap_extension TEXT;
