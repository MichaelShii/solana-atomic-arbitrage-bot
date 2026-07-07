//! Configuration loading module
//!
//! Multi-layer config merging via config crate + dotenvy, priority from low to high:
//!   config.toml → env vars (SOLANA_RPC_URL etc.) → .env file

mod types;
pub use types::*;

use config::{Config, File};
use log::info;

// --- env var override macros ---
// config crate's Environment parses SOLANA_RPC_URL as solana.rpc.url,
// which doesn't match our flat field name rpc_url. Use declarative macros to
// manually override, eliminating repetitive if-let blocks.

macro_rules! apply_env_str {
    ($($var:ident => $path:expr),* $(,)?) => {
        $(if let Ok(v) = std::env::var(stringify!($var)) { $path = v; })*
    };
}

macro_rules! apply_env_parsed {
    ($($var:ident => $path:expr),* $(,)?) => {
        $(if let Ok(v) = std::env::var(stringify!($var)) {
            if let Ok(n) = v.parse() { $path = n; }
        })*
    };
}

/// Load config: config.toml → env var overrides → validate
///
/// Priority (low to high):
///   1. config.toml
///   2. Env vars (SOLANA_RPC_URL, BOT_MODE etc., .env injected via dotenvy)
pub fn load() -> anyhow::Result<AppConfig> {
    // Load order (first wins for dotenvy since it won't override):
    //   .env.{APP_ENV} → .env → .env.local
    // This ensures devnet-specific values take priority over base.
    if let Ok(env) = std::env::var("APP_ENV") {
        if !env.is_empty() {
            let _ = dotenvy::from_filename(format!(".env.{env}"));
        }
    }
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_filename(".env.local");

    // config.toml → config-{APP_ENV}.toml (latter wins for duplicate keys)
    let mut builder = Config::builder()
        .add_source(File::with_name("config.toml").required(false));
    if let Ok(env) = std::env::var("APP_ENV") {
        if !env.is_empty() {
            builder = builder
                .add_source(File::with_name(&format!("config-{env}.toml")).required(false));
        }
    }

    let cfg = builder.build()?;

    let mut app_cfg: AppConfig = cfg.try_deserialize()?;

    apply_env_str! {
        SOLANA_RPC_URL          => app_cfg.solana.rpc_url,
        SOLANA_WS_URL           => app_cfg.solana.ws_url,
        SOLANA_COMMITMENT       => app_cfg.solana.commitment,
        BOT_MODE                => app_cfg.bot.mode,
    }
    apply_env_parsed! {
        SOLANA_RPC_TIMEOUT_SECS       => app_cfg.solana.rpc_timeout_secs,
        BOT_DRY_RUN                   => app_cfg.bot.dry_run,
        RISK_MIN_PROFIT_THRESHOLD_SOL => app_cfg.risk.min_profit_threshold_sol,
        RISK_MAX_SINGLE_INVESTMENT_SOL=> app_cfg.risk.max_single_investment_sol,
        RISK_MAX_DAILY_LOSS_SOL       => app_cfg.risk.max_daily_loss_sol,
        RISK_SLIPPAGE_TOLERANCE_BPS   => app_cfg.risk.slippage_tolerance_bps,
        RISK_MAX_TIP_SOL              => app_cfg.risk.max_tip_sol,
    }

    validate(&app_cfg)?;

    info!(
        "Config loaded: mode={}, rpc_url={}",
        app_cfg.bot.mode, app_cfg.solana.rpc_url,
    );

    Ok(app_cfg)
}

/// Try to read proxy_url from config.toml (for modules that can't access full AppConfig)
pub fn load_config_proxy_url() -> Option<String> {
    let cfg = Config::builder()
        .add_source(File::with_name("config.toml").required(false))
        .build()
        .ok()?;
    cfg.get_string("solana.proxy_url").ok()
}

/// Build a reqwest HTTP client with optional proxy support
pub fn create_http_client(proxy_url: &Option<String>) -> reqwest::Client {
    use std::time::Duration;
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(5));
    if let Some(ref proxy) = proxy_url {
        builder = builder.proxy(reqwest::Proxy::all(proxy).expect("invalid proxy URL"));
    }
    builder.build().expect("failed to build HTTP client")
}

fn validate(cfg: &AppConfig) -> anyhow::Result<()> {
    if cfg.solana.rpc_url.is_empty() {
        anyhow::bail!("solana.rpc_url is empty — set SOLANA_RPC_URL or config.toml");
    }
    // ws_url is optional — main.rs auto-derives it from rpc_url when empty
    if cfg.risk.min_profit_threshold_sol <= 0.0 {
        anyhow::bail!("risk.min_profit_threshold_sol must be > 0");
    }
    if cfg.risk.max_single_investment_sol <= 0.0 {
        anyhow::bail!("risk.max_single_investment_sol must be > 0");
    }
    if cfg.risk.max_daily_loss_sol <= 0.0 {
        anyhow::bail!("risk.max_daily_loss_sol must be > 0");
    }
    if cfg.risk.max_tip_sol <= 0.0 {
        anyhow::bail!("risk.max_tip_sol must be > 0");
    }
    if cfg.dex.pool_fee_bps > 10_000 {
        anyhow::bail!("dex.pool_fee_bps must be <= 10000 (100%)");
    }
    if cfg.dex.pumpfun_fee_bps > 10_000 {
        anyhow::bail!("dex.pumpfun_fee_bps must be <= 10000 (100%)");
    }
    if cfg.dex.pumpswap_fee_bps > 10_000 {
        anyhow::bail!("dex.pumpswap_fee_bps must be <= 10000 (100%)");
    }
    if cfg.scanner.cu_fee_buffer_pct > 1000 {
        anyhow::bail!("scanner.cu_fee_buffer_pct must be <= 1000 (1000%)");
    }
    Ok(())
}

/// Load wallet keypair
///
/// Priority: BOT_PRIVATE_KEY env → wallet.keypair_path
pub fn load_keypair(config: &AppConfig) -> anyhow::Result<solana_sdk::signature::Keypair> {
    if let Ok(b58) = std::env::var("BOT_PRIVATE_KEY") {
        if !b58.is_empty() {
            return Ok(solana_sdk::signature::Keypair::from_base58_string(&b58));
        }
    }
    if let Some(ref path) = config.wallet.keypair_path {
        let path = if path.starts_with('~') {
            if let Ok(home) = std::env::var("HOME") {
                path.replacen('~', &home, 1)
            } else {
                path.clone()
            }
        } else {
            path.clone()
        };
        let keypair = solana_sdk::signer::keypair::read_keypair_file(&path)
            .map_err(|e| anyhow::anyhow!("read keypair file {path}: {e}"))?;
        return Ok(keypair);
    }
    anyhow::bail!(
        "no keypair configured: set BOT_PRIVATE_KEY env or wallet.keypair_path in config.toml"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signer::Signer;

    fn test_config() -> AppConfig {
        AppConfig {
            bot: BotConfig {
                name: "test".into(),
                mode: "live".into(),
                dry_run: true,
            },
            solana: SolanaConfig {
                rpc_url: "https://localhost".into(),
                ws_url: "wss://localhost".into(),
                commitment: "processed".into(),
                rpc_timeout_secs: 10,
                proxy_url: None,
                fallback_rpc_urls: vec![],
                sender_enabled: false,
                sender_swqos_only: true,
                sender_endpoint: String::new(),
            },
            risk: RiskConfig {
                min_profit_threshold_sol: 0.001,
                max_single_investment_sol: 50.0,
                max_daily_loss_sol: 10.0,
                slippage_tolerance_bps: 100,
                max_tip_sol: 0.5,
                blacklist: BlacklistConfig {
                    tokens: vec![],
                    wallets: vec![],
                    programs: vec![],
                },
            },
            dex: DexConfig {
                enabled: vec!["raydium_cpmm".into()],
                min_pool_tvl_sol: 1.0,
                pool_fee_bps: 25,
                pumpfun_fee_bps: 100,
                pumpswap_fee_bps: 25,
            },
            monitoring: MonitoringConfig { metrics_port: 9090 },
            simulator: SimulatorConfig {
                enabled: false,
                wallet_pubkey: String::new(),
            },
            wallet: WalletConfig { keypair_path: None, nonce_account: None },
            arbitrage: ArbitrageConfig::default(),
            scanner: ScannerConfig::default(),
            execution_routing: ExecutionRouting::default(),
            grpc: GrpcConfig::default(),
        }
    }

    #[test]
    fn load_keypair_from_env() {
        let test_kp = solana_sdk::signature::Keypair::new();
        let b58 = test_kp.to_base58_string();
        std::env::set_var("BOT_PRIVATE_KEY", &b58);

        let config = test_config();
        let loaded = load_keypair(&config).expect("should load from BOT_PRIVATE_KEY");
        assert_eq!(loaded.pubkey(), test_kp.pubkey(), "BOT_PRIVATE_KEY");

        std::env::remove_var("BOT_PRIVATE_KEY");
    }

    #[test]
    fn pumpfun_fee_bps_rejects_over_10000() {
        let mut cfg = test_config();
        cfg.dex.pumpfun_fee_bps = 10001;
        let err = validate(&cfg).unwrap_err().to_string();
        assert!(
            err.contains("pumpfun_fee_bps"),
            "expected pumpfun_fee_bps error, got: {err}"
        );
    }

    #[test]
    fn pumpfun_fee_bps_accepts_10000() {
        let mut cfg = test_config();
        cfg.dex.pumpfun_fee_bps = 10000;
        assert!(validate(&cfg).is_ok(), "10000 bps (100%) should be valid");
    }
}
