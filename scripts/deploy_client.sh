#!/bin/bash
# Deploy client binary only (no on-chain program changes).
# Rebuilds and pushes the bot binary to your server.
# Prerequisites:
#   - SSH keypair set up on target server
#   - systemd service `mevbot` configured on target
#   - Edit YOUR_* placeholders below before running.
# Run from project root: ./scripts/deploy_client.sh
set -e
git pull 2>/dev/null || true
cargo build --release
scp -i ~/.ssh/YOUR_SSH_KEY.pem target/release/mevbot YOUR_USER@YOUR_SERVER_IP:/path/to/mevbot/mevbot.new
ssh -i ~/.ssh/YOUR_SSH_KEY.pem YOUR_USER@YOUR_SERVER_IP "
  sudo systemctl stop mevbot
  cp /path/to/mevbot/mevbot /path/to/mevbot/mevbot.bak
  mv /path/to/mevbot/mevbot.new /path/to/mevbot/mevbot
  chmod +x /path/to/mevbot/mevbot
  sudo systemctl start mevbot
  sleep 5
  sudo journalctl -u mevbot --since '3 sec ago' --no-pager | tail -5
"
echo "Done! Bot restarted."
