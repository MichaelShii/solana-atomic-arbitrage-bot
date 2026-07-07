#!/bin/bash
# Full deploy: rebuild on-chain program + deploy program + deploy client.
# WARNING: This spends real SOL (buffer rent ~0.4 SOL + tx fees).
# Prerequisites:
#   - programs/arbitrage/deploy/funder.json (deploy wallet with SOL)
#   - programs/arbitrage/deploy/program-id.json (your program keypair)
#   - Solana CLI configured, cargo-build-sbf installed
#   - Edit YOUR_* placeholders below before running.
# Run from project root: ./scripts/deploy_full.sh
set -euo pipefail
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

echo "=== Building SBF (on-chain program) ==="
cd "$PROJECT_DIR/programs/arbitrage"
cargo build-sbf --features mainnet
cd "$PROJECT_DIR"

echo "=== Building client ==="
cargo build --release --bin mevbot

echo "=== Deploying on-chain program ==="
BUFFER=$(solana program write-buffer programs/arbitrage/target/deploy/arbitrage.so \
  --keypair programs/arbitrage/deploy/funder.json \
  --url "https://api.mainnet-beta.solana.com" \
  --with-compute-unit-price 100000 2>&1 | grep 'Buffer:' | awk '{print $2}')
echo "Buffer: $BUFFER"

solana program deploy --buffer "$BUFFER" \
  --program-id programs/arbitrage/deploy/program-id.json \
  --keypair programs/arbitrage/deploy/funder.json \
  --url "https://api.mainnet-beta.solana.com"

echo "=== Verifying ==="
solana program show YOUR_PROGRAM_ID \
  --url "https://api.mainnet-beta.solana.com"

echo "=== Deploying client ==="
scp -i ~/.ssh/YOUR_SSH_KEY.pem target/release/mevbot YOUR_USER@YOUR_SERVER_IP:/path/to/mevbot/mevbot.new
ssh -i ~/.ssh/YOUR_SSH_KEY.pem YOUR_USER@YOUR_SERVER_IP "
  sudo systemctl stop mevbot
  cp /path/to/mevbot/mevbot /path/to/mevbot/mevbot.bak
  mv /path/to/mevbot/mevbot.new /path/to/mevbot/mevbot
  chmod +x /path/to/mevbot/mevbot
  sudo systemctl start mevbot
"
echo "=== Done ==="
