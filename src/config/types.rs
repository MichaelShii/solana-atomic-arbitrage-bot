//! Config type definitions
//!
//! Java analogy: Spring Boot's @ConfigurationProperties + application.yml
//! Difference: Rust uses serde to generate serialization code at compile time, no runtime reflection

use serde::Deserialize;

/// Top-level app config structure
/// #[derive(Deserialize)] ≈ Jackson @JsonDeserialize, but generated at compile time
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub bot: BotConfig,
    pub solana: SolanaConfig,
    pub risk: RiskConfig,
    pub dex: DexConfig,
    pub simulator: SimulatorConfig,
    pub monitoring: MonitoringConfig,
    #[serde(default)]
    pub wallet: WalletConfig,
    #[serde(default)]
    pub arbitrage: ArbitrageConfig,
    #[serde(default)]
    pub scanner: ScannerConfig,
    #[serde(default)]
    pub execution_routing: ExecutionRouting,
    #[serde(default)]
    pub grpc: GrpcConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BotConfig {
    // Reserved for future multi-bot deployment
    #[allow(dead_code)]
    pub name: String,
    pub mode: String,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SolanaConfig {
    pub rpc_url: String,
    /// WebSocket URL. If left empty, derived automatically from rpc_url
    /// (https:// → wss://, http:// → ws://). Only set this if your provider
    /// uses a different WebSocket host.
    #[serde(default)]
    pub ws_url: String,
    // Reserved for future commitment-level policy
    #[serde(default = "default_commitment")]
    #[allow(dead_code)]
    pub commitment: String,
    #[serde(default = "default_rpc_timeout")]
    pub rpc_timeout_secs: u64,
    /// HTTP(S) proxy for all outbound RPC/API requests.
    /// WSL users behind a proxy (Clash, v2ray, etc.) should set this.
    /// Format: "http://127.0.0.1:7897"
    #[serde(default)]
    pub proxy_url: Option<String>,
    /// Fallback RPC endpoints tried in order when the primary fails.
    /// Example: ["https://rpc1.example.com", "https://rpc2.example.com"]
    #[serde(default)]
    pub fallback_rpc_urls: Vec<String>,
    /// Helius Sender for ultra-low-latency TX submission (dual routing to validators + Jito).
    /// When enabled, submit_via_sender is tried first; falls back to standard RPC on failure.
    #[serde(default)]
    pub sender_enabled: bool,
    /// Use SWQOS-only routing (no Jito auction). Tip drops from 0.0002 → 0.000005 SOL.
    #[serde(default = "default_true")]
    #[allow(dead_code)]
    pub sender_swqos_only: bool,
    /// Sender regional endpoint for low-latency TX submission.
    #[serde(default = "default_sender_endpoint")]
    #[allow(dead_code)]
    pub sender_endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RiskConfig {
    pub min_profit_threshold_sol: f64,
    pub max_single_investment_sol: f64,
    pub max_daily_loss_sol: f64,
    pub slippage_tolerance_bps: u32,
    // Reserved for future Jito tip caps and runtime blacklisting
    #[allow(dead_code)]
    pub max_tip_sol: f64,
    #[allow(dead_code)]
    pub blacklist: BlacklistConfig,
}

// All fields reserved — blacklist filtering not yet wired into the event loop.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BlacklistConfig {
    #[serde(default)]
    pub tokens: Vec<String>,
    #[serde(default)]
    pub wallets: Vec<String>,
    #[serde(default)]
    pub programs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DexConfig {
    // Reserved: per-venue enable/disable and TVL filtering
    #[serde(default = "default_enabled_dexes")]
    #[allow(dead_code)]
    pub enabled: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub min_pool_tvl_sol: f64,
    #[serde(default = "default_pool_fee_bps")]
    pub pool_fee_bps: u32,
    /// Pump.fun bonding curve protocol fee rate (basis points), default 100 = 1%
    #[serde(default = "default_pumpfun_fee_bps")]
    pub pumpfun_fee_bps: u32,
    /// PumpSwap AMM protocol fee rate (basis points), default 25 = 0.25%
    #[serde(default = "default_pumpswap_fee_bps")]
    pub pumpswap_fee_bps: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MonitoringConfig {
    #[serde(default = "default_metrics_port")]
    pub metrics_port: u16,
}

/// Pool scanner config — replaces the original Listener + Watchlist
#[derive(Debug, Clone, Deserialize)]
pub struct ScannerConfig {
    // Reserved: scanner loop pacing, pool refresh, and batch size limits
    /// Price difference scan interval (milliseconds)
    #[serde(default = "default_scan_interval_ms")]
    #[allow(dead_code)]
    pub scan_interval_ms: u64,
    /// Full pool list refresh interval (seconds)
    #[serde(default = "default_pool_refresh_interval_secs")]
    #[allow(dead_code)]
    pub pool_refresh_interval_secs: u64,
    /// Max tokens to process per scan
    #[serde(default = "default_max_pools_per_scan")]
    #[allow(dead_code)]
    pub max_pools_per_scan: usize,
    /// Minimum price difference (basis points)
    #[serde(default = "default_min_price_diff_bps")]
    pub min_price_diff_bps: u32,
    /// Minimum pool liquidity (SOL)
    #[serde(default = "default_min_pool_liquidity_sol")]
    pub min_pool_liquidity_sol: f64,
    /// ComputeBudget unit price (micro-lamports per CU)
    #[serde(default = "default_cu_price")]
    pub compute_unit_price_micro_lamports: u64,
    /// ComputeBudget unit limit
    #[serde(default = "default_cu_limit")]
    pub compute_unit_limit: u32,
    /// Profit safety factor: only execute when net_profit × factor ≥ threshold
    /// 0.85 = 15% safety margin, to account for slippage estimation error and on-chain volatility
    #[serde(default = "default_profit_safety_factor")]
    pub profit_safety_factor: f64,
    /// CU fee buffer percentage: actual_required_SOL = cu_cost × (1 + buffer_pct/100)
    /// 20% = pad CU estimate by 20%, prevent insufficient balance from on-chain CU price fluctuations
    #[serde(default = "default_cu_fee_buffer_pct")]
    pub cu_fee_buffer_pct: u32,
    /// ATA creation rent reserve (SOL), covers new ATAs that may be created in the arbitrage path
    #[serde(default = "default_ata_rent_reserve_sol")]
    pub ata_rent_reserve_sol: f64,
    /// Skip Phase A DLMM bin re-read verification (saves 80-200ms).
    /// Phase C build still re-reads latest data, so this is a safe optimization.
    #[serde(default)]
    pub skip_reverify: bool,
    /// Max pool share per transaction (0.02 = 2%)
    #[serde(default = "default_max_pool_share")]
    pub max_pool_share: f64,
    /// Max absolute SOL output per transaction
    #[serde(default = "default_max_absolute_sol_out")]
    pub max_absolute_sol_out: f64,
    /// Enable background active scanning (gRPC cache, 2s interval). Off by default; enable when needed.
    #[serde(default)]
    pub active_scan_enabled: bool,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            scan_interval_ms: default_scan_interval_ms(),
            pool_refresh_interval_secs: default_pool_refresh_interval_secs(),
            max_pools_per_scan: default_max_pools_per_scan(),
            min_price_diff_bps: default_min_price_diff_bps(),
            min_pool_liquidity_sol: default_min_pool_liquidity_sol(),
            compute_unit_price_micro_lamports: default_cu_price(),
            compute_unit_limit: default_cu_limit(),
            profit_safety_factor: default_profit_safety_factor(),
            cu_fee_buffer_pct: default_cu_fee_buffer_pct(),
            ata_rent_reserve_sol: default_ata_rent_reserve_sol(),
            skip_reverify: false,
            max_pool_share: default_max_pool_share(),
            max_absolute_sol_out: default_max_absolute_sol_out(),
            active_scan_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SimulatorConfig {
    /// Whether to enable on-chain simulation verification
    #[serde(default)]
    pub enabled: bool,
    /// Our wallet address (used for ATA derivation and simulation)
    #[serde(default)]
    pub wallet_pubkey: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WalletConfig {
    /// Private key file path (Solana CLI JSON format, contains 64-byte keypair)
    pub keypair_path: Option<String>,
    /// Durable nonce account address (base58).
    /// When set, TXs use durable nonce instead of latest blockhash,
    /// eliminating getLatestBlockhash RPC from the critical path.
    #[serde(default)]
    #[allow(dead_code)]
    pub nonce_account: Option<String>,
}

// --- Default value functions ---

fn default_commitment() -> String {
    "processed".into()
}

fn default_rpc_timeout() -> u64 {
    30
}

fn default_enabled_dexes() -> Vec<String> {
    vec![
        "pump_fun".into(),
        "meteora_dlmm".into(),
        "raydium_ammv4".into(),
        "raydium_cpmm".into(),
    ]
}

fn default_metrics_port() -> u16 {
    9090
}

fn default_scan_interval_ms() -> u64 {
    1000
}

fn default_pool_refresh_interval_secs() -> u64 {
    120
}

fn default_max_pools_per_scan() -> usize {
    50
}

fn default_min_price_diff_bps() -> u32 {
    30
}

fn default_min_pool_liquidity_sol() -> f64 {
    5.0
}

fn default_max_pool_share() -> f64 {
    0.02
}

fn default_max_absolute_sol_out() -> f64 {
    2.0
}

fn default_cu_price() -> u64 {
    50_000
}

fn default_cu_limit() -> u32 {
    600_000
}

fn default_profit_safety_factor() -> f64 {
    0.85
}

fn default_cu_fee_buffer_pct() -> u32 {
    20
}

fn default_ata_rent_reserve_sol() -> f64 {
    0.005
}

fn default_pool_fee_bps() -> u32 {
    25
}

fn default_pumpfun_fee_bps() -> u32 {
    100
}

fn default_pumpswap_fee_bps() -> u32 {
    25
}

fn default_pumpfun_fee_recipient() -> String {
    "CebN5WGQ4jvEPvsVU4EoHEpgT1mKQ7AFUbxmAhvFUWrQ".into()
}

fn default_true() -> bool {
    true
}

fn default_sender_endpoint() -> String {
    "http://your-sender-endpoint.example.com".into()
}

// ============================================================
// Cross-pool arbitrage config
// ============================================================

#[derive(Debug, Clone, Deserialize)]
pub struct ArbitrageConfig {
    /// Pump.fun protocol fee recipient address (read from global account, this is a fallback)
    #[serde(default = "default_pumpfun_fee_recipient")]
    pub pumpfun_fee_recipient: String,
}

impl Default for ArbitrageConfig {
    fn default() -> Self {
        Self {
            pumpfun_fee_recipient: default_pumpfun_fee_recipient(),
        }
    }
}

/// On-chain arbitrage program routing config.
///
/// Hash-based canary: `(token_mint.as_bytes()[0] as u64) % 100 < onchain_traffic_pct`
/// routes the trade to the on-chain program; otherwise falls back to legacy builders.
#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionRouting {
    /// Master switch — must be true for any on-chain routing to occur.
    #[serde(default)]
    pub use_onchain_program: bool,
    /// Percentage of traffic to route on-chain (0..=100).
    #[serde(default)]
    pub onchain_traffic_pct: u8,
    /// On-chain arbitrage program ID.
    #[serde(default = "default_onchain_program_id")]
    pub onchain_program_id: String,
    /// Address Lookup Table for compressing on-chain TX size.
    /// Created once via `solana address-lookup-table create`.
    #[serde(default)]
    pub onchain_arb_alt: Option<String>,
}

impl Default for ExecutionRouting {
    fn default() -> Self {
        Self {
            use_onchain_program: false,
            onchain_traffic_pct: 0,
            onchain_program_id: default_onchain_program_id(),
            onchain_arb_alt: None,
        }
    }
}

fn default_onchain_program_id() -> String {
    "YOUR_PROGRAM_ID".into()
}

// ============================================================
// gRPC config
// ============================================================

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GrpcConfig {
    /// Yellowstone gRPC endpoint (e.g. https://grpc.example.com)
    #[serde(default)]
    pub endpoint: String,
    /// x-token for authentication
    #[serde(default)]
    pub x_token: String,
}

