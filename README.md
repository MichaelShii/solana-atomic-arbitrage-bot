# MEVbot — Solana Multi-DEX Atomic Arbitrage

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[中文文档](README.zh-CN.md)

12-route cross-DEX arbitrage bot. Executes buy+sell in a single Solana TX via self-deployed on-chain Router with generic orchestrator, leveraging TX atomicity for all-or-nothing execution.

## Project Status

**Production — 12 routes active, CPMM/Whirlpool pool cache expanding**

| Module | Status | Description |
|--------|--------|-------------|
| `programs/arbitrage/` | ✅ Deployed | On-chain Router — 2 legacy + 8 generic routes via `ROUTE_DISC` |
| `executor/atomic/` | ✅ | Atomic TX builder, 5 builder modules (split from monolith) |
| `simulator/` | ✅ | PumpSwap/DLMM/CPMM instruction builders, DLMM limit-order estimation |
| `pool_cache/` | ✅ | CPMM/DLMM/Whirlpool pool reserves + persistence |
| `arbitrage/` | ✅ | 4-venue price scanner + golden-section search + TTL cache |
| `listener/` | ✅ | WebSocket listener: PumpSwap + DLMM + CPMM + Whirlpool (4 subs) |
| `grpc_stream/` | ✅ | Yellowstone gRPC (token expired, RPC fallback active) |
| `risk/` | ✅ | Runtime risk (daily loss circuit breaker, single-trade cap, balance) |
| `main_loop/` | ✅ | Event-driven loop + blacklist + slot freshness + RPC failover |
| `confirmation/` | ✅ | Background confirmation (PnL extraction, estimate vs actual) |
| `persistence/` | ✅ | SQLite (whitelist, DLMM/CPMM/Whirlpool metadata, TP cache, trades) |
| `whitelist/` | ✅ | Multi-venue whitelist (≥2 venues with pools → verified) |

## Supported Venues & Routes

| | PumpSwap | DLMM | CPMM | Whirlpool |
|--|----------|------|------|-----------|
| **PumpSwap** | — | ✅ | ✅ | ✅ |
| **DLMM** | ✅ | — | ✅ | ✅ |
| **CPMM** | ✅ | ✅ | — | ✅ |
| **Whirlpool** | ✅ | ✅ | ✅ | — |

**12 routes** — 6 pair × 2 directions. Legacy PumpSwap↔DLMM via dedicated handlers; remaining 8 via generic orchestrator (`ROUTE_DISC`).

## Architecture

```
WebSocket listener (4 subscriptions: PumpSwap + DLMM + CPMM + Whirlpool)
    │
    ├─ main_loop: event-driven
    │   ├─ verify_dual_presence: ≥2 venues with pools → whitelist
    │   ├─ Scanner: 4-venue price query (parallel, TTL cache)
    │   │   ├─ PumpSwap → gRPC/RPC
    │   │   ├─ DLMM    → gRPC/RPC
    │   │   ├─ CPMM    → RPC fetch (persisted pool addrs)
    │   │   └─ Whirlpool → RPC fetch (persisted pool addrs)
    │   │
    │   ├─ R2-M01 freshness: re-fetch reserves before building
    │   ├─ H-02 WSOL: ensure balance, fire-and-forget wrap if needed
    │   ├─ Builder: v0 TX with ALT (27 addresses)
    │   ├─ Simulate (optional): pre-flight check before submit
    │   └─ Submit: sendTransaction (skip_preflight)
    │       └─ H-03 RPC Pool: Helius → Shyft → QuickNode (round-robin)
    │
    └─ programs/arbitrage (on-chain Router)
        ├─ Legacy: route_pump_to_dlmm, route_dlmm_to_pump
        └─ Generic orchestrator (ROUTE_DISC)
            ├─ dex_pumpswap.rs  → PumpSwap CPI (buy/sell)
            ├─ dex_dlmm.rs      → DLMM swap2 CPI
            ├─ dex_cpmm.rs      → CPMM swap CPI
            └─ dex_whirlpool.rs → Whirlpool swap CPI
```

## On-Chain Router

- **Generic orchestrator** (`orchestrate.rs`): validate → snapshot → buy CPI → read intermediate → sell CPI → post-invariants
- **DEX identification**: `identify_dex()` probes offset 0 (CPMM/Whirlpool/DLMM) then offset 16 (PumpSwap)
- **12 CPI invocations**: 4 DEX × (buy + sell legs) with M-02 error logging
- **Account layout varies by DEX**:

| DEX | Fixed accounts | Program offset |
|-----|---------------|----------------|
| PumpSwap buy | 23 + remaining | 16 |
| PumpSwap sell | 23 (padded from 21) | 16 |
| DLMM | 13 + bin arrays (extended for mints/programs) | 0 |
| CPMM | 13 | 0 |
| Whirlpool | 12 + tick arrays | 0 |

## Infrastructure

| Feature | Description |
|---------|-------------|
| **H-02 WSOL Replenishment** | Runtime balance check, fire-and-forget wrap with `WRAP_IN_FLIGHT` guard |
| **H-03 RPC Pool** | Multi-endpoint round-robin with auto-failover on error |
| **APP_ENV switching** | `APP_ENV=devnet` loads `config-devnet.toml` + `.env.devnet` overlay |
| **Pool persistence** | CPMM/Whirlpool pool addrs stored in SQLite, survive restarts |

## Quick Start

### Prerequisites

- Rust toolchain (see `rust-toolchain.toml`)
- A Solana RPC endpoint (free tier from Helius/QuickNode/Shyft works for testing)
- A Solana wallet with a small amount of SOL (~0.01 for testing)
- Solana CLI (only needed if deploying the on-chain program)

### Minimal Setup (dry-run, 2 minutes)

This mode scans pools and prints opportunities but never submits transactions. Safe to run with any wallet.

```bash
git clone https://github.com/MichaelShii/solana-atomic-arbitrage-bot.git
cd solana-atomic-arbitrage-bot

# Copy templates
cp .env.example .env
cp config.example.toml config.toml

# Edit .env — only these 2 are required:
#   SOLANA_RPC_URL=https://your-rpc-provider.com
#   BOT_PRIVATE_KEY=your_base58_private_key

# Build (5-10 min first time) and run
cargo build --release
./target/release/mevbot
```

On startup you'll see a config summary — verify your RPC endpoint and profit thresholds are correct.

### Going Live

To actually execute trades, change two settings:

```toml
# config.toml
[bot]
dry_run = false       # was true

[risk]
min_profit_threshold_sol = 0.0001   # adjust to your tolerance
max_single_investment_sol = 0.5     # start small
```

For lower-latency submission, add your Helius API key to `.env`:
```
HELIUS_API_KEY=your_key
```

### Devnet Testing

```bash
APP_ENV=devnet cargo run --release --bin mevbot
```

### Optional: Deploy On-Chain Router

The bot works without deploying your own on-chain program. However, deploying the included Router reduces transaction size and improves execution success rate. See [docs/ONCHAIN_DEPLOYMENT.md](docs/ONCHAIN_DEPLOYMENT.md) for instructions.

### Configuration Reference

| Variable | Location | Required | Description |
|----------|----------|----------|-------------|
| `SOLANA_RPC_URL` | `.env` or `config.toml` `[solana].rpc_url` | Yes | Solana RPC endpoint |
| `SOLANA_WS_URL` | `.env` or `config.toml` `[solana].ws_url` | Yes | Solana WebSocket endpoint |
| `BOT_PRIVATE_KEY` | `.env` | Yes | Base58-encoded 64-byte keypair |
| `HELIUS_API_KEY` | `.env` | No | Helius API key (Sender + gRPC) |
| `SHYFT_API_KEY` | `.env` | No | Shyft gRPC x-token |
| `[wallet].keypair_path` | `config.toml` | Alternative | Path to Solana CLI keypair JSON |
| `[wallet].nonce_account` | `config.toml` | No | Durable nonce account address |
| `[execution_routing].onchain_program_id` | `config.toml` | No | Your deployed arbitrage program ID |
| `[execution_routing].onchain_arb_alt` | `config.toml` | No | Address Lookup Table address |

## Module Structure

```
src/
├── main.rs                  Entry point + mode dispatch + wallet
├── constants.rs             All Program IDs / Mints / Discriminators
├── config/                  Multi-layer config (toml + env)
├── executor/
│   ├── atomic/
│   │   ├── mod.rs           TX build & submit dispatch (12 route match arms)
│   │   ├── onchain_router.rs   Legacy builders + shared helpers + ALT cache
│   │   ├── generic_route.rs    Section data types + build_generic_route + pricing
│   │   ├── builders_legacy.rs    pump↔dlmm TX builders
│   │   ├── builders_cpmm_wp.rs   cpmm↔whirlpool + pump↔cpmm TX builders
│   │   ├── builders_pump_dlmm.rs dlmm↔whirlpool + pump↔whirlpool + cpmm↔dlmm
│   │   └── helpers.rs       PumpSwap meta + reserves
│   ├── rpc_pool.rs          Round-robin RPC pool (H-03)
│   └── confirmation.rs      Background PnL confirmation
├── simulator/               Instruction builders (PumpSwap, DLMM, CPMM)
├── pool_cache/              Pool reserves (CPMM, DLMM, Whirlpool, BondingCurve)
├── arbitrage/               Scanner + price queries + golden-section search
├── listener/                WebSocket (4 program subscriptions)
├── risk/                    Circuit breaker + balance guard
└── main_loop.rs             Event loop + verify_dual_presence + H-02 WSOL

programs/arbitrage/          On-chain Router (SBF)
├── src/
│   ├── lib.rs               Instruction dispatch (3 discriminators)
│   ├── constants.rs         PDA seeds, account indices, DEX kind IDs
│   ├── error.rs             Error codes 6000-6500
│   ├── accounting.rs        SOL/token balance snapshots
│   ├── cpi/
│   │   ├── pump_swap.rs     PumpSwap buy/sell CPI
│   │   ├── dlmm.rs          DLMM swap2 CPI
│   │   ├── cpmm.rs          Raydium CPMM swap CPI
│   │   └── whirlpool.rs     Orca Whirlpool swap CPI
│   └── instructions/
│       ├── orchestrate.rs      Generic 2-leg orchestrator (ROUTE_DISC)
│       ├── dex_pumpswap.rs     PumpSwap handler + validator
│       ├── dex_dlmm.rs         DLMM handler + validator
│       ├── dex_cpmm.rs         CPMM handler + validator
│       ├── dex_whirlpool.rs    Whirlpool handler + validator
│       ├── route_pump_to_dlmm.rs  Legacy pump→dlmm
│       └── route_dlmm_to_pump.rs  Legacy dlmm→pump
```

## Key Documents

- [Deployment Guide](docs/DEPLOYMENT.md)
- [On-Chain Deployment](docs/ONCHAIN_DEPLOYMENT.md)

## Disclaimer

**Risk Warning**: This software executes real financial transactions on Solana mainnet. You can lose money. Before running with real funds:

1. **Start in dry-run mode** (`dry_run = true` in `config.toml`) — scans and simulates without submitting transactions
2. **Test on devnet** first (`APP_ENV=devnet`) with small amounts
3. **Understand the risks**: sandwich attacks, slippage, failed transactions, MEV competition, RPC latency
4. **Never commit secrets**: `.env`, `config.toml`, keypair files, and deploy artifacts are git-ignored
5. **Use a dedicated wallet**: never use your main wallet; fund only what you can afford to lose

This project is for educational and research purposes only. The authors assume no responsibility for financial losses, transaction failures, or any other consequences arising from the use of this software.
