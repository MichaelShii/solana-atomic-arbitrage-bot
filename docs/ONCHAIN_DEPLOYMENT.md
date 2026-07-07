# On-Chain Program Deployment & Upgrade Guide

Last updated: 2026-06-18

This guide documents the full build, deploy, and upgrade workflow for the MEVbot on-chain arbitrage program. The program is native Solana (no Anchor); each deployment/upgrade requires manual commands. No need to re-research for new sessions.

## Overview

| Item | Value |
|------|-------|
| Program directory | `programs/arbitrage/` |
| Program entry | `programs/arbitrage/src/lib.rs` |
| Framework | native Solana (`solana-program`), not Anchor |
| Program ID (mainnet) | `YOUR_MAINNET_PROGRAM_ID` |
| Program ID (devnet) | `YOUR_DEVNET_PROGRAM_ID` |
| Program Keypair | `programs/arbitrage/target/deploy/arbitrage-keypair.json` |
| Binary (.so) | `programs/arbitrage/target/deploy/arbitrage.so` |
| Client config reference | `onchain_program_id` in `config.toml` |
| ALT | `onchain_arb_alt` in `config.toml` (created via `scripts/create_alt.sh`, contains 27 fixed addresses) |
| Deploy wallet (mainnet) | `programs/arbitrage/deploy/funder.json` (keypair file, permissions `-rw-------`) |
| Deploy wallet (devnet) | `programs/arbitrage/deploy/devnet-keypair.json` |
| Bot wallet | `BOT_PRIVATE_KEY` in `.env` (**different** from the deploy wallet) |

## Prerequisites

```bash
# Confirm Solana CLI is installed and version >= 4.0
solana --version
# Expected: solana-cli 4.0.x

# Confirm cargo-build-sbf is available
cargo build-sbf --version
# Expected: cargo-build-sbf 4.0.x, platform-tools v1.53
```

Solana CLI and build-sbf toolchain install path: `~/.local/share/solana/install/active_release/bin/`

## Building

```bash
cd programs/arbitrage

# Mainnet build (default arch, no --arch v3)
cargo build-sbf --features mainnet

# Devnet build (must use --arch v3)
cargo build-sbf --arch v3
```

**SBPF version differences**:

| Environment | Build command | ELF e_flags | sbpf version |
|------|---------|-------------|--------------|
| Mainnet | `cargo build-sbf` (default) | `0x0` | SBFv0 |
| Devnet | `cargo build-sbf --arch v3` | `0x3` | SBFv3 |

Verify ELF header: `readelf -h target/deploy/arbitrage.so | grep flags`

- Mainnet **does not accept** SBFv3 (e_flags=0x3) binaries — will error `sbpf_version required by the executable which are not enabled`
- Devnet 4.1.0-beta.1 **only** enables SBPFv3; does not accept default arch binaries
- **Deploying the wrong arch .so to mainnet wastes one buffer creation + close gas fee**

`--features mainnet` enables conditional compilation, switching `OUR_ARBITRAGE_PROGRAM_ID` from the devnet placeholder to the real mainnet address.

Output files:
- `target/deploy/arbitrage.so` — deployable program binary
- `target/deploy/arbitrage-keypair.json` — program keypair (auto-generated on first build, **do not delete**)

Behavior of `cargo build-sbf`:
- If `target/deploy/arbitrage-keypair.json` already exists → reuse it; the .so output keeps the same program ID
- If not present → auto-generate a new keypair, producing a brand new program ID
- **Therefore**: as long as the keypair file is not deleted, every build produces a .so with the same program ID

### Toolchain Version Requirements

Currently depends on `solana-program 1.18.26` → `blake3 1.8.5` → `digest 0.11` → `ctutils` → `cmov 0.5.4`. cmov requires Edition 2024 (MSRV 1.85).

| tools-version | rustc | Edition 2024 | v4 lockfile | Usable |
|:---|:---|:---|:---|:---|
| v1.53 (default) | 1.89.0 | Supported | Supported | Yes |
| v1.41 | 1.75.0 | Not supported | Not supported | No |

If the parent workspace has a v4 `Cargo.lock`, using v1.41 will error `lock file version 4 requires -Znext-lockfile-bump`. Temporarily moving the parent lockfile away can bypass this, but v1.41 also cannot handle Edition 2024 crates (will error `feature 'edition2024' is required`). Conclusion: **must use v1.53+**.

The `Cargo.toml` release profile configures `opt-level = "s"`, `lto = true`, `codegen-units = 1`, `panic = "abort"` to minimize binary size.

## Running Tests

```bash
cd programs/arbitrage
cargo test
```

Integration tests cover CPI account layout verification, ensuring consistency with real mainnet Swap2 transactions.

## Program Keypair Explanation

The program keypair is **not** the wallet that pays deployment fees. In the `solana program deploy` command:
- `--program-id <keypair>` — specifies the program keypair, which determines the program address
- `--keypair <keypair>` or default keypair — specifies the fee payer (pays rent + deployment costs)

In this project:
- Program keypair: `target/deploy/arbitrage-keypair.json` (program identity)
- Fee payer (mainnet): `deploy/funder.json` (pays fees)
- Fee payer (devnet): `deploy/devnet-keypair.json`

The public key corresponding to the program keypair (not the program ID):
```
$ solana-keygen pubkey programs/arbitrage/target/deploy/arbitrage-keypair.json
YOUR_KEYPAIR_PUBKEY
```

After deployment, the Solana runtime generates the actual program address `YOUR_MAINNET_PROGRAM_ID` based on the keypair.

Warning: **The program keypair must not be lost or deleted** — without it you cannot upgrade the program. `cargo clean` does not delete keypair files under `target/deploy/`, but manual deletion permanently loses upgrade authority.

## Deployment (First Time)

First deployment uploads the program on-chain; Solana charges rent for the program buffer + executable account (approximately 3-4 SOL).

```bash
# === Mainnet ===
# 1. Confirm RPC URL points to mainnet
solana config set --url https://api.mainnet-beta.solana.com
# Or use your dedicated RPC endpoint

# 2. Deploy
solana program deploy \
  --program-id programs/arbitrage/target/deploy/arbitrage-keypair.json \
  --keypair programs/arbitrage/deploy/funder.json \
  --with-compute-unit-price 100000 \
  programs/arbitrage/target/deploy/arbitrage.so

# 3. Record the output Program ID, update config.toml
#   [execution_routing]
#   onchain_program_id = "<PROGRAM_ID>"

# === Devnet ===
# devnet must use --arch v3 built .so (see build section above)
solana config set --url https://api.devnet.solana.com

solana program deploy \
  --program-id programs/arbitrage/deploy/devnet-program-keypair.json \
  --keypair programs/arbitrage/deploy/devnet-keypair.json \
  --with-compute-unit-price 100000 \
  programs/arbitrage/target/deploy/arbitrage.so
```

Post-deployment verification:
```bash
solana program show YOUR_MAINNET_PROGRAM_ID
```

Output should show program owner as `BPFLoaderUpgradeab1e11111111111111111111111`, data length approximately 60KB.

## Upgrading (Updating a Deployed Program)

The upgrade command is **exactly the same** as first deployment — the Solana CLI automatically detects the program ID already exists and follows the upgrade path.

### Warning: `--program-id` must point to the keypair of the **currently on-chain program**

There are **two** program keypairs in the project, corresponding to two different program IDs:

| Keypair file | Corresponding program ID | Purpose |
|-------------|------------|------|
| `deploy/program-id.json` | Your mainnet program address | **Mainnet deployed program** |
| `target/deploy/arbitrage-keypair.json` | Auto-generated by first `cargo build-sbf` | Local build keypair |

**Using the wrong keypair triggers a new deployment instead of an upgrade**, creating a new program address and costing approximately 0.35 SOL in buffer rent. If this happens, use `solana program close <BUFFER>` to recover lamports.

```bash
# Mainnet upgrade — correct command
solana program deploy \
  --program-id programs/arbitrage/deploy/program-id.json \
  --keypair programs/arbitrage/deploy/funder.json \
  --url "https://api.mainnet-beta.solana.com" \
  programs/arbitrage/target/deploy/arbitrage.so

# Devnet upgrade
solana program deploy \
  --program-id programs/arbitrage/deploy/devnet-program-keypair.json \
  --keypair programs/arbitrage/deploy/devnet-keypair.json \
  --url "https://api.devnet.solana.com" \
  programs/arbitrage/target/deploy/arbitrage.so
```

### Buffer Rent & Balance Requirements

During the upgrade flow, the CLI first creates a buffer account to store the .so content, then executes the upgrade. The buffer needs to be rent-exempt:

```
buffer_rent = (binary_size + 128) x 6960 lamports
            ~ 59688+128 = 59816 x 6960 ~ 0.416 SOL
```

The funder needs to cover buffer rent + at least 2-3 transaction fees (create buffer, write, upgrade). **Recommended funder balance >= buffer_rent + 0.01 SOL** (approximately 0.36 SOL for the current 50KB program).

Current funder: `YOUR_FUNDER_ADDRESS`

Notes:
- Upgrading does not change the program ID — `onchain_program_id` in `config.toml` does not need modification
- Upgrading does not change the ALT — unless fixed accounts are added/removed, `onchain_arb_alt` does not need modification
- Confirm deploy/funder.json has sufficient SOL (see calculation above)
- The bot wallet (`BOT_PRIVATE_KEY` in `.env`) and deploy wallet (funder) are **different** accounts

## ALT (Address Lookup Table) Management

ALT is used to compress transaction size — 22 fixed addresses are placed in the ALT, referenced in transactions by index (1 byte each) instead of full pubkeys (32 bytes each).

### Creating an ALT

```bash
bash scripts/create_alt.sh
```

What the script does:
1. Reads `BOT_PRIVATE_KEY` from `.env` as fee payer
2. Creates an empty ALT
3. Extends with 22 fixed addresses
4. Outputs the ALT address

The 27-address list is hardcoded in `scripts/create_alt.sh` and must be kept **strictly in sync** with `alt_fixed_addresses()` in `onchain_router.rs` — when modifying one side, synchronize the other side as well.

### When to Rebuild the ALT

When the following occurs:
- Any fixed address changes after an upgrade (e.g. PumpSwap upgrade)
- Adding/removing venues requires adding/removing fixed addresses
- ALT is full (currently 27/256)

After rebuilding, update `onchain_arb_alt` in `config.toml`.

Warning: `onchain_router.rs:alt_fixed_addresses()` and `scripts/create_alt.sh` must remain in sync. When modifying, change both sides together.

## Relationship Between Deployment and Client Configuration

The client (`config.toml`) configures which on-chain program to use:

```toml
[execution_routing]
onchain_program_id = "YOUR_PROGRAM_ID"
onchain_arb_alt = "YOUR_ALT_ADDRESS"
onchain_traffic_pct = 100   # 100% routed via on-chain path
```

Client code paths:
- `src/executor/atomic/onchain_router.rs` — builds routing instructions, contains `alt_fixed_addresses()`
- `src/executor/atomic/mod.rs` — detect_token_program + calls onchain_router
- `src/constants.rs` — all address constant definitions

Program side:
- `programs/arbitrage/src/instructions/` — route handlers
- `programs/arbitrage/src/cpi/` — CPI builders (dlmm, pump_swap, token)
- `programs/arbitrage/src/constants.rs` — program-side constants (**independent of** client `src/constants.rs`, must be kept in sync manually)

## Post-Upgrade Verification Checklist

1. `cargo test` all pass
2. Program deployed/upgraded successfully (`solana program show <PID>`)
3. **Canary rollout**: verify startup on devnet first, then mainnet `onchain_traffic_pct = 1` (1% canary)
4. Observe logs for 30 minutes to confirm no CPI errors (`ARB_PUMP_CPI_FAILED`, `ARB_CPMM_CPI_FAILED`, etc.)
5. Gradually ramp up if no issues: `traffic_pct: 1 -> 5 -> 25 -> 100`
6. CPMM/Whirlpool routes do not need canary rollout — there is no legacy fallback path; they auto-activate once pool cache data is available

## Devnet Environment

| Item | Value |
|:---|:---|
| Devnet program ID | `YOUR_DEVNET_PROGRAM_ID` |
| Devnet program keypair | `programs/arbitrage/deploy/devnet-program-keypair.json` |
| Devnet deploy wallet | `programs/arbitrage/deploy/devnet-keypair.json` |
| Devnet bot wallet | `programs/arbitrage/deploy/devnet-bot-keypair.json` (`YOUR_DEVNET_BOT_ADDRESS`) |
| Devnet ALT | `YOUR_DEVNET_ALT_ADDRESS` |
| Devnet cluster version | 4.1.0-beta.1 |
| Devnet enabled features | SBPFv3 activated, SBPFv2 incompatible |

### Build Command

```bash
cargo build-sbf --arch v3   # devnet must use v3; mainnet must NOT use v3, use default arch
```

## Known Pitfalls

### `alloc::format` — cargo check reports unused but cannot be removed

`use alloc::format;` in `programs/arbitrage/src/lib.rs` reports unused import under `cargo check`, but the `entrypoint!` macro expansion requires it during SBF compilation. Add `#[allow(unused_imports)]` + a comment explaining the reason.

### Wrong `--program-id` keypair = new deployment instead of upgrade

Two keypairs correspond to two different program IDs (see upgrade section comparison table). `solana program deploy` does not check "did you mean to upgrade?" — it only checks whether the address for `--program-id` already exists. Wrong keypair -> address does not exist -> new deployment -> wastes ~0.35 SOL buffer rent.

### Verify ELF header before upgrading

Before deployment, confirm the binary targets the correct chain:
```bash
readelf -h target/deploy/arbitrage.so | grep flags
# e_flags=0x0 -> mainnet compatible
# e_flags=0x3 -> devnet compatible
```

### `OUR_ARBITRAGE_PROGRAM_ID` is conditionally compiled

In `programs/arbitrage/src/constants.rs`:
```rust
#[cfg(feature = "mainnet")]
pub const OUR_ARBITRAGE_PROGRAM_ID: Pubkey = /* mainnet address */;
#[cfg(not(feature = "mainnet"))]
pub const OUR_ARBITRAGE_PROGRAM_ID: Pubkey = /* devnet address */;
```
`--features mainnet` only takes effect at **build time** and does not affect runtime behavior. Forgetting `--features mainnet` causes the program to embed the devnet address (currently this constant is not referenced by on-chain logic, but may be used in CPI self-checks in the future).

### Clean up buffers on deployment failure

A failed deployment leaves a buffer account on-chain (costing approximately 0.35 SOL) which must be manually recovered:

```bash
# List all buffers
solana program show --buffers --keypair deploy/devnet-keypair.json --url <rpc>

# Close a specific buffer; lamports are returned to the authority wallet
solana program close <BUFFER_ADDRESS> --keypair deploy/devnet-keypair.json --url <rpc>
```

## Troubleshooting

| Error | Possible cause |
|------|---------|
| `Error: Insufficient funds` | deploy/funder.json balance too low (buffer rent ~0.352 SOL + tx fees), see balance calculation in upgrade section |
| `Account allocation failed: account does not have enough SOL` | funder balance < buffer rent, usually short by 0.001-0.01 SOL. Transfer 0.02 SOL from bot wallet |
| `sbpf_version ... not enabled` | Binary arch does not match target chain — mainnet used `--arch v3` or devnet did not use `--arch v3` |
| `Error: Account in use` | Program ID conflict |
| `Error: failed to send transaction` | RPC congestion, increase `--with-compute-unit-price` or switch RPC endpoint |
| Wrong `--program-id` keypair causes new deployment | See keypair comparison table in "Upgrading" section above. Use `solana program close <BUFFER>` to recover |
| CPI errors (WrongProgram, ConstraintMut) | Account layout mismatch; check indices in `alt_fixed_addresses()` and CPI builders |
| `cargo check` reports `unused import: alloc::format` | **Do not remove** — `entrypoint!` macro needs it during SBF compilation. Add `#[allow(unused_imports)]` |

## Key File Quick Reference

```
programs/arbitrage/
├── Cargo.toml                        # Program dependencies (solana-program 1.17-1.19)
├── src/
│   ├── lib.rs                        # Entry: process_instruction + instruction dispatch
│   ├── constants.rs                  # Program-side constants (discriminators, indices)
│   ├── error.rs                      # Custom error types
│   ├── instructions/
│   │   ├── mod.rs                    # Instruction dispatch
│   │   ├── route_dlmm_to_pump.rs     # DLMM -> PumpSwap route (legacy)
│   │   ├── route_pump_to_dlmm.rs     # PumpSwap -> DLMM route (legacy)
│   │   ├── orchestrate.rs            # Generic 2-leg orchestrator (ROUTE_DISC)
│   │   ├── dex_cpmm.rs               # CPMM handler (validate + CPI wrapper)
│   │   └── dex_whirlpool.rs          # Whirlpool handler (validate + CPI wrapper)
│   └── cpi/
│       ├── dlmm.rs                   # DLMM Swap2 CPI builder
│       ├── pump_swap.rs              # PumpSwap buy/sell CPI builder
│       ├── cpmm.rs                   # Raydium CPMM swap CPI builder
│       ├── whirlpool.rs              # Orca Whirlpool swap CPI builder
│       └── token.rs                  # Token transfer CPI helpers
├── target/deploy/
│   ├── arbitrage.so                  # Compiled binary (~60KB)
│   └── arbitrage-keypair.json        # Program keypair (232 bytes)
└── deploy/
    ├── funder.json                   # Mainnet deploy wallet (keep secret)
    ├── devnet-keypair.json           # Devnet deploy wallet
    ├── devnet-program-keypair.json   # Devnet program keypair
    └── devnet-program-id.txt         # Devnet program ID plaintext record

scripts/
└── create_alt.sh                     # Create Address Lookup Table

src/
├── constants.rs                      # Client constants (independent of program side; sync addresses manually)
├── config/types.rs                   # AppConfig definition
└── executor/atomic/
    ├── onchain_router.rs             # Client route builder + ALT definition
    └── mod.rs                        # Calls onchain_router
```

## Important Notes

- **The program side and client side each have their own `constants.rs`** — they are independent files. When modifying addresses, keep both sides in sync.
- **The program binary and keypair are tracked in git** (`programs/arbitrage/target/deploy/` is not excluded in `.gitignore`). The hex content of the keypair is visible in git log — ensure it is not pushed to a public repository.
- The deploy wallet (`deploy/funder.json`) and `BOT_PRIVATE_KEY` in `.env` are **different** accounts. Deploy uses funder; bot execution uses the key in `.env`.
- `cargo build-sbf` uses a specific toolchain version (configurable via `--tools-version`); verify compatibility on major version upgrades.
