# Deployment Guide

## Prerequisites

- Ubuntu 22.04+ server (2 vCPU / 4GB RAM)
- Direct connection to Solana RPC (latency < 100ms)
- Rust toolchain installed locally for cross-compilation

## Build

Build locally (server memory typically insufficient for `cargo build --release`):

```bash
cargo build --release
```

## Server Setup

### 1. Upload

```bash
scp target/release/mevbot config.toml .env user@<YOUR_SERVER_IP>:~/mevbot/
```

### 2. Configuration

Copy `config.example.toml` to `config.toml` and fill in:

- `solana.rpc_url` / `solana.ws_url` — your RPC provider
- `[grpc]` section — Yellowstone gRPC endpoint for real-time pool state
- `[scanner]` — adjust thresholds for your strategy
- Optional: set `solana.fallback_rpc_urls` for failover

Create `.env` with secrets:

```
SOLANA_RPC_URL=https://rpc.example.com?api-key=YOUR_KEY
SOLANA_WS_URL=wss://ws.example.com?api-key=YOUR_KEY
BOT_PRIVATE_KEY=your_base58_private_key
```

### 3. Run

```bash
cd ~/mevbot
nohup ./mevbot > mevbot.log 2>&1 &
```

Logs written to `~/.local/share/mevbot/mevbot.log` (JSON format, daily rotation).

## Systemd (Optional)

```ini
[Unit]
Description=MEVbot
After=network.target

[Service]
Type=simple
User=ubuntu
WorkingDirectory=/opt/mevbot
EnvironmentFile=/opt/mevbot/.env
ExecStart=/opt/mevbot/mevbot
Restart=always
RestartSec=30

[Install]
WantedBy=multi-user.target
```

## Data

- SQLite: `~/.local/share/mevbot/mevbot.db`
- Logs: `~/.local/share/mevbot/mevbot.log.YYYY-MM-DD`
- Whitelist persists in SQLite across restarts

## Dry-run

Set `dry_run = true` to scan without submitting transactions. Wallet still required for simulation.
