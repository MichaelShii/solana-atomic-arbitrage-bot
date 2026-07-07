# MEVbot — Solana 多 DEX 原子套利机器人

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[English](README.md)

12 条路由的跨 DEX 套利机器人。通过自部署的链上 Router 在单笔 Solana 交易内完成买入+卖出，利用交易原子性实现全有或全无执行。

## 项目状态

**生产环境 — 12 条路由已启用，CPMM/Whirlpool 池子缓存持续扩充中**

| 模块 | 状态 | 说明 |
|--------|--------|-------------|
| `programs/arbitrage/` | ✅ 已部署 | 链上 Router — 2 条遗留路由 + 8 条通用路由 (via `ROUTE_DISC`) |
| `executor/atomic/` | ✅ | 原子交易构建器，5 个构建模块 |
| `simulator/` | ✅ | PumpSwap/DLMM/CPMM 指令构建，DLMM 限价单估算 |
| `pool_cache/` | ✅ | CPMM/DLMM/Whirlpool 池子储备 + 持久化 |
| `arbitrage/` | ✅ | 四市场价差扫描 + 黄金分割搜索 + TTL 缓存 |
| `listener/` | ✅ | WebSocket 监听：PumpSwap + DLMM + CPMM + Whirlpool |
| `grpc_stream/` | ✅ | Yellowstone gRPC |
| `risk/` | ✅ | 运行时风控（日亏损熔断、单笔上限、余额检查） |
| `main_loop/` | ✅ | 事件驱动主循环 + 黑名单 + slot 新鲜度 + RPC 故障转移 |
| `confirmation/` | ✅ | 后台确认（PnL 提取，预估 vs 实际对比） |
| `persistence/` | ✅ | SQLite（白名单、DLMM/CPMM/Whirlpool 元数据、TP 缓存、交易记录） |
| `whitelist/` | ✅ | 多市场白名单（≥2 个市场有池 → 验证通过） |

## 支持的市场与路由

| | PumpSwap | DLMM | CPMM | Whirlpool |
|--|----------|------|------|-----------|
| **PumpSwap** | — | ✅ | ✅ | ✅ |
| **DLMM** | ✅ | — | ✅ | ✅ |
| **CPMM** | ✅ | ✅ | — | ✅ |
| **Whirlpool** | ✅ | ✅ | ✅ | — |

**12 条路由** — 6 对 × 双向。PumpSwap↔DLMM 走专用处理器；其余 8 条通过通用编排器 (`ROUTE_DISC`)。

## 架构

```
WebSocket 监听 (4 个订阅: PumpSwap + DLMM + CPMM + Whirlpool)
    │
    ├─ main_loop: 事件驱动
    │   ├─ verify_dual_presence: ≥2 个市场有池 → 加入白名单
    │   ├─ Scanner: 四市场价格查询 (并行, TTL 缓存)
    │   │   ├─ PumpSwap → gRPC/RPC
    │   │   ├─ DLMM    → gRPC/RPC
    │   │   ├─ CPMM    → RPC 查询 (持久化池地址)
    │   │   └─ Whirlpool → RPC 查询 (持久化池地址)
    │   │
    │   ├─ R2-M01 新鲜度: 构建前重取最新储备
    │   ├─ H-02 WSOL: 检查余额，必要时 fire-and-forget wrap
    │   ├─ Builder: v0 交易 + ALT (27 个地址)
    │   ├─ Simulate (可选): 提交前预检
    │   └─ Submit: sendTransaction (skip_preflight)
    │       └─ H-03 RPC Pool: 多端点轮询 + 自动故障转移
    │
    └─ programs/arbitrage (链上 Router)
        ├─ Legacy: route_pump_to_dlmm, route_dlmm_to_pump
        └─ Generic orchestrator (ROUTE_DISC)
            ├─ dex_pumpswap.rs  → PumpSwap CPI (买/卖)
            ├─ dex_dlmm.rs      → DLMM swap2 CPI
            ├─ dex_cpmm.rs      → CPMM swap CPI
            └─ dex_whirlpool.rs → Whirlpool swap CPI
```

## 链上 Router

- **通用编排器** (`orchestrate.rs`): 校验 → 快照 → 买入 CPI → 读取中间状态 → 卖出 CPI → 后验不变量
- **DEX 识别**: `identify_dex()` 先探测 offset 0 (CPMM/Whirlpool/DLMM)，再探测 offset 16 (PumpSwap)
- **12 次 CPI 调用**: 4 个 DEX × (买入 + 卖出)，含错误日志
- **账户布局因 DEX 而异**:

| DEX | 固定账户数 | Program offset |
|-----|-----------|----------------|
| PumpSwap 买入 | 23 + remaining | 16 |
| PumpSwap 卖出 | 23 (从 21 补齐) | 16 |
| DLMM | 13 + bin arrays (已含 mints/programs) | 0 |
| CPMM | 13 | 0 |
| Whirlpool | 12 + tick arrays | 0 |

## 基础设施

| 特性 | 说明 |
|---------|-------------|
| **H-02 WSOL 补充** | 运行时余额检查，fire-and-forget wrap + `WRAP_IN_FLIGHT` 守卫 |
| **H-03 RPC 池** | 多端点轮询 + 自动故障转移 |
| **APP_ENV 切换** | `APP_ENV=devnet` 加载 `config-devnet.toml` + `.env.devnet` |
| **池子持久化** | CPMM/Whirlpool 池地址存 SQLite，重启不丢失 |

## 快速开始

### 前提条件

- Rust 工具链（见 `rust-toolchain.toml`）
- 一个 Solana RPC 端点（Helius/QuickNode/Shyft 免费套餐即可测试）
- 一个有小额 SOL 的 Solana 钱包（~0.01 SOL 用于测试）
- Solana CLI（仅部署链上程序时需要）

### 最小配置（dry-run 模式，2 分钟）

此模式只扫描池子并打印套利机会，不会提交任何交易。

```bash
git clone https://github.com/MichaelShii/solana-atomic-arbitrage-bot.git
cd solana-atomic-arbitrage-bot

cp .env.example .env
cp config.example.toml config.toml

# 编辑 .env，只需填这 2 个：
#   SOLANA_RPC_URL=https://你的RPC地址
#   BOT_PRIVATE_KEY=你的base58私钥

cargo build --release
./target/release/mevbot
```

启动后会打印配置摘要 — 确认 RPC 端点和利润阈值正确即可。

### 实盘交易

```toml
# config.toml
[bot]
dry_run = false       # 原来是 true

[risk]
min_profit_threshold_sol = 0.0001
max_single_investment_sol = 0.5     # 建议从小额开始
```

如需低延迟提交，在 `.env` 中添加：`HELIUS_API_KEY=你的key`

### 可选：部署链上 Router

不部署链上程序也可以正常使用。详见 [docs/ONCHAIN_DEPLOYMENT.md](docs/ONCHAIN_DEPLOYMENT.md)。

### 配置参考

| 变量 | 位置 | 必填 | 说明 |
|----------|----------|----------|-------------|
| `SOLANA_RPC_URL` | `.env` | 是 | Solana RPC 端点 |
| `BOT_PRIVATE_KEY` | `.env` | 是 | Base58 编码的 64 字节私钥 |
| `HELIUS_API_KEY` | `.env` | 否 | Helius API key（低延迟提交） |
| `SHYFT_API_KEY` | `.env` | 否 | Shyft gRPC x-token |
| `[wallet].keypair_path` | `config.toml` | 替代方案 | Solana CLI keypair JSON 路径 |
| `[execution_routing].onchain_program_id` | `config.toml` | 否 | 你部署的链上程序 ID |

### 什么需要改，什么不需要

**需要你配置的**（都在 `config.toml` 和 `.env` 里）：
- RPC 端点、钱包私钥、利润阈值、滑点、链上程序 ID

**绝对不能改的**（`src/constants.rs`，全网统一）：
- Raydium/PumpSwap/Orca/Meteora 等 DEX 的程序 ID
- WSOL、USDC 等 Token Mint 地址
- Anchor 指令 discriminator（sha256 哈希值）
- PDA seeds、指令布局偏移量

详见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## 模块结构

```
src/
├── main.rs                  入口 + 模式分发 + 钱包
├── constants.rs             所有 Program ID / Mint / Discriminator
├── config/                  多层配置 (toml + env)
├── executor/
│   ├── atomic/
│   │   ├── mod.rs           交易构建与提交分发 (12 路由匹配)
│   │   ├── onchain_router.rs   遗留构建器 + 共享工具 + ALT 缓存
│   │   ├── generic_route.rs    通用路由数据类型 + 构建 + 定价
│   │   ├── builders_legacy.rs    pump↔dlmm 交易构建
│   │   ├── builders_cpmm_wp.rs   cpmm↔whirlpool + pump↔cpmm
│   │   ├── builders_pump_dlmm.rs dlmm↔whirlpool + pump↔whirlpool + cpmm↔dlmm
│   │   └── helpers.rs        PumpSwap meta + 储备
│   ├── rpc_pool.rs          轮询 RPC 池 (H-03)
│   └── confirmation.rs      后台 PnL 确认
├── simulator/               指令构建器 (PumpSwap, DLMM, CPMM)
├── pool_cache/              池子储备 (CPMM, DLMM, Whirlpool, BondingCurve)
├── arbitrage/               扫描器 + 价格查询 + 黄金分割搜索
├── listener/                WebSocket (4 个程序订阅)
├── risk/                    熔断 + 余额守护
└── main_loop.rs             事件循环 + verify_dual_presence + H-02 WSOL

programs/arbitrage/          链上 Router (SBF)
├── src/
│   ├── lib.rs               指令分发 (3 个 discriminator)
│   ├── constants.rs         PDA seeds, 账户索引, DEX 类型 ID
│   ├── error.rs             错误码 6000-6500
│   ├── accounting.rs        SOL/token 余额快照
│   ├── cpi/
│   │   ├── pump_swap.rs     PumpSwap 买/卖 CPI
│   │   ├── dlmm.rs          DLMM swap2 CPI
│   │   ├── cpmm.rs          Raydium CPMM swap CPI
│   │   └── whirlpool.rs     Orca Whirlpool swap CPI
│   └── instructions/
│       ├── orchestrate.rs      通用 2-leg 编排器 (ROUTE_DISC)
│       ├── dex_pumpswap.rs     PumpSwap 处理器 + 校验
│       ├── dex_dlmm.rs         DLMM 处理器 + 校验
│       ├── dex_cpmm.rs         CPMM 处理器 + 校验
│       ├── dex_whirlpool.rs    Whirlpool 处理器 + 校验
│       ├── route_pump_to_dlmm.rs  遗留 pump→dlmm
│       └── route_dlmm_to_pump.rs  遗留 dlmm→pump
```

## 相关文档

- [部署指南](docs/DEPLOYMENT.md)
- [链上部署](docs/ONCHAIN_DEPLOYMENT.md)

## 免责声明

**风险警告**: 本软件在 Solana 主网上执行真实金融交易。你可能亏钱。在使用真实资金前:

1. **先用 dry-run 模式** (`dry_run = true`) — 只扫描和模拟，不提交交易
2. **先在 devnet 上测试** (`APP_ENV=devnet`)
3. **理解风险**: 三明治攻击、滑点、交易失败、MEV 竞争、RPC 延迟
4. **不要提交密钥**: `.env`、`config.toml`、keypair 文件、部署产物已加入 `.gitignore`
5. **使用独立钱包**: 绝不要用主钱包；只转入你能承受亏损的金额

本项目仅供教育和研究用途。作者对因使用本软件导致的任何财务损失、交易失败或其他后果不承担任何责任。
