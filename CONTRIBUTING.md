# Contributing

## Prerequisites

- Rust toolchain (see `rust-toolchain.toml` for version)
- Solana CLI: <https://docs.solana.com/cli/install-solana-cli-tools>
- A Solana RPC endpoint (any provider: Helius, QuickNode, Shyft, Triton, etc.)

## First-Time Setup (5 minutes)

### Step 1 — Create config files

```bash
cp config.example.toml config.toml
cp .env.example .env
```

### Step 2 — Fill in `.env`

| Key | Required? | Where to get it |
|-----|-----------|-----------------|
| `SOLANA_RPC_URL` | **Yes** | Your RPC provider dashboard |
| `SOLANA_WS_URL` | **Yes** | Same provider, WSS endpoint |
| `BOT_PRIVATE_KEY` | **Yes** | `solana-keygen new` or export from wallet |
| `HELIUS_API_KEY` | Optional | Helius dashboard (for Sender) |
| `SHYFT_API_KEY` | Optional | Shyft dashboard (for gRPC) |

### Step 3 — Fill in `config.toml`

Only **3 sections** typically need changes:

| Section | Key | Notes |
|---------|-----|-------|
| `[solana]` | `rpc_url`, `ws_url` | Can leave empty if using `.env` |
| `[wallet]` | `keypair_path` | Alternative to `BOT_PRIVATE_KEY` env |
| `[risk]` | `min_profit_threshold_sol`, `max_single_investment_sol` | Adjust to your risk tolerance |

All other sections use safe defaults. Start with `dry_run = true`.

### Step 4 — (Optional) Deploy on-chain program

If you want on-chain routing (lower TX size, higher success rate):

1. Edit `programs/arbitrage/src/constants.rs` — replace `OUR_ARBITRAGE_PROGRAM_ID` with `YOUR_PROGRAM_ID`
2. Build: `cd programs/arbitrage && cargo build-sbf --features mainnet`
3. Deploy: `solana program deploy ...` (see `scripts/deploy_full.sh`)
4. Set `config.toml`:
   ```toml
   [execution_routing]
   use_onchain_program = true
   onchain_program_id = "YOUR_DEPLOYED_PROGRAM_ID"
   ```

The client reads the program ID from config. The on-chain program needs it at compile time (Solana BPF requirement).

### Step 5 — Run (dry-run first!)

```bash
cargo build --release --bin mevbot
./target/release/mevbot    # dry_run=true: scans without submitting
```

## What NOT to edit

The file `src/constants.rs` contains **public protocol addresses** (Raydium, PumpSwap, Orca, Meteora program IDs, mints like WSOL/USDC, PDA seeds, instruction discriminators). These are the same for everyone on Solana mainnet — no need to change them.

The on-chain program `programs/arbitrage/src/constants.rs` is similar: only the `OUR_ARBITRAGE_PROGRAM_ID` placeholder at the bottom needs your attention.

## Code Style

- `cargo fmt` and `cargo clippy` before submitting
- On-chain program uses SBF target: `cargo build-sbf` in `programs/arbitrage/`
- No hardcoded secrets — use `config.toml` or environment variables

## Disclaimer

This project is for educational and research purposes only. Mainnet trading may result in significant financial loss. The authors assume no responsibility for consequences arising from the use of this software.
