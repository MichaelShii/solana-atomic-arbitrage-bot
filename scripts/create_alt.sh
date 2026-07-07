#!/bin/bash
# Create Address Lookup Table for on-chain arbitrage TX compression.
# Run once, paste the output address into config.toml [execution_routing] onchain_arb_alt.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# -- Load BOT_PRIVATE_KEY from .env --
BOT_KEY=$(grep BOT_PRIVATE_KEY "$PROJECT_DIR/.env" | cut -d= -f2-)
if [ -z "$BOT_KEY" ]; then
    echo "ERROR: BOT_PRIVATE_KEY not found in .env"
    exit 1
fi

# -- Create temp keypair file --
TMP_KEYPAIR=$(mktemp /tmp/mevbot_alt_keypair_XXXXXX.json)
python3 -c "
import json

# Pure base58 decode (no external deps)
alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
def b58decode(s):
    n = 0
    for c in s:
        n = n * 58 + alphabet.index(c)
    ba = []
    while n > 0:
        n, r = divmod(n, 256)
        ba.append(r)
    ba.reverse()
    for c in s:
        if c == '1': ba.insert(0, 0)
        else: break
    return bytes(ba)

pk_bytes = b58decode('$BOT_KEY')
with open('$TMP_KEYPAIR', 'w') as f:
    json.dump(list(pk_bytes), f)
"
echo "Temp keypair created: $TMP_KEYPAIR"

# -- Fixed addresses (must match alt_fixed_addresses() order in onchain_router.rs) --
# -- NOTE: any change here MUST be reflected in alt_fixed_addresses() and vice versa. --
ADDRESSES=(
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"         # 0: TOKEN_PROGRAM
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"         # 1: ATA_PROGRAM
    "11111111111111111111111111111111"                      # 2: SYSTEM_PROGRAM
    "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"          # 3: PUMPFUN_AMM_PROGRAM
    "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo"          # 4: DLMM_PROGRAM
    "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"          # 5: MEMO_PROGRAM
    "ComputeBudget111111111111111111111111111111"            # 6: COMPUTE_BUDGET_PROGRAM
    "So11111111111111111111111111111111111111112"           # 7: NATIVE_SOL_MINT
    "ADyA8hdefvWN2dbGGWFotbzWxrAvLW83WG6QCVXvJKqw"          # 8: PUMPSWAP_GLOBAL_CONFIG
    "GS4CU59F31iL7aR2Q8zVS8DRrcRnXX1yjQ66TqNVQnaR"          # 9: PUMPSWAP_EVENT_AUTHORITY
    "C2aFPdENg4A2HQsmrd5rTw5TaYBX5Ku887cWjbFKtZpw"          # 10: PUMPSWAP_GLOBAL_VOLUME_ACCUMULATOR
    "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ"          # 11: PUMPSWAP_FEE_PROGRAM
    "5YxQFdt3Tr9zJLvkFccqXVUwhdTWJQc1fFg2YPbxvxeD"          # 12: PUMPSWAP_BUYBACK_FEE_RECIPIENT
    "D1ZN9Wj1fRSUQfCjhvnu1hqDMT7hzjzBBpi12nVniYD6"          # 13: DLMM_EVENT_AUTHORITY
    "62qc2CNXwrYqQScmEdiZFFAnJR262PxWEuNQtxfafNgV"          # 14: PUMPSWAP_PROTOCOL_FEE_RECIPIENTS[0]
    "GesfTA3X2arioaHp8bbKdjG9vJtskViWACZoYvxp4twS"          # 15: PUMPSWAP_RESERVED_FEE_RECIPIENTS[0]
    "5PHirr8joyTMp9JMm6nW7hNDVyEYdkzDqazxPD7RaTjx"           # 16: fee_config PDA
    "SysvarRent111111111111111111111111111111111"            # 17: SYSVAR_RENT
    "94qWNrtmfn42h3ZjUZwWvK1MEo9uVmmrBPd2hpNjYDjb"          # 18: protocol_fee_ata (ATA of [14] + SOL)
    "HjQjngTDqoHE6aaGhUqfz9aQ7WZcBRjy5xB8PScLSr8i"          # 19: buyback_ata (ATA of [12] + SOL)
    "C93K8DX4YsABYJtHX9awzgZW3LWzBqBVezEbbLJH4yet"          # 20: reserved_fee_ata (ATA of [15] + SOL)
    "92tvs8JMjxgoyGSPMMKseVSMk2SUsiRwNxdffd5QYQHz"          # 21: global_vol_accum_ata (ATA of [10] + SOL)
)

# -- Create ALT --
ADDR_STR=$(IFS=,; echo "${ADDRESSES[*]}")
set +e  # disable exit-on-error for solana CLI calls

echo ""
echo "Step 1/2: Creating empty ALT..."
echo ""

CREATE_RESULT=$(solana address-lookup-table create \
    --keypair "$TMP_KEYPAIR" 2>&1)
CREATE_EXIT=$?

ALT_ADDRESS=$(echo "$CREATE_RESULT" | grep -oP '(?:Lookup Table Address|address):\s*\K[1-9A-HJ-NP-Za-km-z]+' | head -1)

if [ $CREATE_EXIT -ne 0 ] || [ -z "$ALT_ADDRESS" ]; then
    echo "ERROR: failed to create ALT (exit=$CREATE_EXIT):"
    echo "$CREATE_RESULT"
    rm -f "$TMP_KEYPAIR"
    exit 1
fi

echo "ALT created: $ALT_ADDRESS"
echo ""
echo "Step 2/2: Extending ALT with ${#ADDRESSES[@]} addresses (expect 22)..."
echo ""

EXTEND_RESULT=$(solana address-lookup-table extend "$ALT_ADDRESS" \
    --keypair "$TMP_KEYPAIR" \
    --addresses "$ADDR_STR" 2>&1)
EXTEND_EXIT=$?

rm -f "$TMP_KEYPAIR"

if [ $EXTEND_EXIT -ne 0 ]; then
    echo "ERROR: extend failed (exit=$EXTEND_EXIT):"
    echo "$EXTEND_RESULT"
    exit 1
fi

echo ""
echo "=== ALT READY ==="
echo "Address: $ALT_ADDRESS"
echo ""
echo "Add this line to config.toml under [execution_routing]:"
echo "  onchain_arb_alt = \"$ALT_ADDRESS\""
