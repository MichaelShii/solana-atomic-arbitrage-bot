#!/usr/bin/env python3
"""
DLMM Pool Warmup: For each mint in the whitelist, query lb_pair via GPAv2,
parse bin_step, and cache into the SQLite dlmm_metadata table.
Reports mints with issues.
"""

import base64
import json
import os
import sqlite3
import struct
import sys
import time
import urllib.request

# minimal base58 — no external deps
_B58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

def b58encode(data: bytes) -> str:
    n = int.from_bytes(data, "big")
    if n == 0:
        return _B58_ALPHABET[0]
    s = []
    while n > 0:
        n, r = divmod(n, 58)
        s.append(_B58_ALPHABET[r])
    # leading zeros
    for b in data:
        if b == 0:
            s.append(_B58_ALPHABET[0])
        else:
            break
    return "".join(reversed(s))

DLMM_PROGRAM = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo"
SOL_MINT = "So11111111111111111111111111111111111111112"

def db_path():
    home = os.environ.get("HOME", ".")
    return os.path.join(home, ".local/share/mevbot/mevbot.db")

def load_rpc_url():
    if os.path.exists(".env"):
        with open(".env") as f:
            for line in f:
                if line.startswith("SOLANA_RPC_URL="):
                    return line.strip().split("=", 1)[1]
    sys.exit("ERROR: SOLANA_RPC_URL not found (check .env)")

def load_whitelist():
    conn = sqlite3.connect(db_path())
    rows = conn.execute("SELECT mint FROM whitelist WHERE category != 'blacklisted'").fetchall()
    conn.close()
    return [r[0] for r in rows]

def load_metadata():
    conn = sqlite3.connect(db_path())
    rows = conn.execute("SELECT key, lb_pair, token_x_mint, token_y_mint, bin_step, base_factor, bin_array_bitmap_extension FROM dlmm_metadata").fetchall()
    conn.close()
    metadata = {}
    for key, lb_pair, tx, ty, bs, bf, bmpx in rows:
        metadata.setdefault(key, []).append({
            "lb_pair": lb_pair, "token_x_mint": tx,
            "token_y_mint": ty, "bin_step": bs,
            "base_factor": bf,
            "bin_array_bitmap_extension": bmpx,
        })
    return metadata

def save_metadata(metadata):
    conn = sqlite3.connect(db_path())
    # Incremental write: INSERT OR REPLACE only writes new/updated key->lb_pair mappings
    count = 0
    for key, entries in metadata.items():
        for e in entries:
            conn.execute(
                "INSERT OR REPLACE INTO dlmm_metadata (key, lb_pair, token_x_mint, token_y_mint, bin_step, base_factor, bin_array_bitmap_extension) VALUES (?, ?, ?, ?, ?, ?, ?)",
                (key, e["lb_pair"], e["token_x_mint"], e["token_y_mint"], e["bin_step"], e["base_factor"], e.get("bin_array_bitmap_extension")),
            )
            count += 1
    conn.commit()
    conn.close()
    print(f"[SAVED] {count} entries -> {db_path()}")

def parse_lb_pair(pubkey, data_b64):
    """Extract token_x_mint, token_y_mint, bin_step, base_factor, bin_array_bitmap_extension from base64-encoded lb_pair data."""
    data = base64.b64decode(data_b64)
    if len(data) < 152:
        return None

    # offset 88-119: token_x_mint (32 bytes)
    # offset 120-151: token_y_mint (32 bytes)
    # offset 80-82: bin_step (u16 LE)
    # offset 84-86: base_factor (u16 LE)
    token_x_bytes = data[88:120]
    token_y_bytes = data[120:152]
    bin_step = struct.unpack_from("<H", data, 80)[0]
    base_factor = struct.unpack_from("<H", data, 84)[0]

    token_x_mint = b58encode(token_x_bytes)
    token_y_mint = b58encode(token_y_bytes)

    # offset 248: bin_array_bitmap_extension (Borsh Option<Pubkey>)
    # 1-byte tag (0=None, 1=Some) + optional 32-byte pubkey
    bin_array_bitmap_extension = None
    if len(data) >= 281 and data[248] == 1:
        bin_array_bitmap_extension = b58encode(data[249:281])

    return {
        "lb_pair": pubkey,
        "token_x_mint": token_x_mint,
        "token_y_mint": token_y_mint,
        "bin_step": bin_step,
        "base_factor": base_factor,
        "bin_array_bitmap_extension": bin_array_bitmap_extension,
    }

def query_gpav2(rpc_url, mint_x, mint_y):
    body = json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccountsV2",
        "params": [
            DLMM_PROGRAM,
            {
                "encoding": "base64",
                "limit": 1000,
                "filters": [
                    {"memcmp": {"offset": 88, "bytes": mint_x}},
                    {"memcmp": {"offset": 120, "bytes": mint_y}},
                ]
            }
        ]
    }).encode()

    req = urllib.request.Request(
        rpc_url,
        data=body,
        headers={"Content-Type": "application/json"}
    )

    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            data = json.loads(resp.read())
    except Exception as e:
        return None, f"HTTP: {e}"

    if "error" in data:
        return None, f"RPC: {data['error']}"

    result = data.get("result", {})
    if isinstance(result, dict):
        accounts = result.get("accounts", [])
        pagination_key = result.get("paginationKey")
    elif isinstance(result, list):
        accounts = result
        pagination_key = None
    else:
        return None, f"unexpected result type: {type(result)}"

    return accounts, pagination_key

def main():
    rpc_url = load_rpc_url()
    print(f"=== DLMM Pool Warmup ===")
    print(f"RPC: {rpc_url[:60]}...")
    print(f"DB: {db_path()}")

    mints = load_whitelist()
    metadata = load_metadata()
    total = len(mints)
    print(f"{total} mints total, {len(metadata)} keys already cached\n")

    found = []
    empty = []
    errors = []
    skipped = 0

    for i, mint in enumerate(mints):
        short = mint[:10]
        head = f"[{i+1}/{total}] {short}"

        key_a = f"{mint}:{SOL_MINT}"
        key_b = f"{SOL_MINT}:{mint}"

        if key_a in metadata or key_b in metadata:
            print(f"{head} (cached, skip)")
            skipped += 1
            continue

        # Query both mint orderings in parallel
        accounts_a, err_a = query_gpav2(rpc_url, mint, SOL_MINT)
        accounts_b, err_b = query_gpav2(rpc_url, SOL_MINT, mint)

        if err_a or err_b:
            error_detail = err_a or err_b
            print(f"{head} [ERROR] {error_detail}")
            errors.append(mint)
            continue

        count_a = len(accounts_a) if accounts_a else 0
        count_b = len(accounts_b) if accounts_b else 0
        total_pools = count_a + count_b

        if total_pools == 0:
            print(f"{head} [EMPTY] 0 DLMM pools")
            empty.append(mint)
        else:
            print(f"{head} [OK] {total_pools} pools (meme=x: {count_a}, meme=y: {count_b})")
            found.append(mint)
            # Parse accounts, append to metadata (supports multiple pools per mint-pair)
            for accounts in (accounts_a, accounts_b):
                for acct in accounts:
                    pubkey = acct.get("pubkey", "")
                    data_b64 = acct.get("account", {}).get("data", [""])[0]
                    if not pubkey or not data_b64:
                        continue
                    entry = parse_lb_pair(pubkey, data_b64)
                    if entry is None:
                        continue
                    key = f"{entry['token_x_mint']}:{entry['token_y_mint']}"
                    if key not in metadata:
                        metadata[key] = []
                    # Dedup: don't add the same lb_pair twice
                    if not any(e["lb_pair"] == entry["lb_pair"] for e in metadata[key]):
                        metadata[key].append(entry)

        time.sleep(0.15)

    # Write back to DB
    save_metadata(metadata)

    print(f"\n=== Results ===")
    print(f"With pools: {len(found)}")
    print(f"No pools:   {len(empty)}")
    print(f"Errors:     {len(errors)}")
    print(f"Skipped:    {skipped} (cached)")

    if empty:
        print(f"\n--- Mints with no DLMM pools ({len(empty)}) ---")
        for m in empty:
            print(f"  {m}")

    if errors:
        print(f"\n--- Mints with query errors ({len(errors)}) ---")
        for m in errors:
            print(f"  {m}")

if __name__ == "__main__":
    main()
