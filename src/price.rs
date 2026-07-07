//! SOL/USDC price (multi-source fallback + single-source retry, 60s refresh, RwLock singleton)
//!
//! Price source priority: Pyth (Hermes REST API) → Binance → Bybit → Okx → Jupiter → CoinGecko
//! Pyth Hermes and CEX APIs all use HTTPS; WSL network environment may block some domains

use log::{debug, info, warn};
use std::sync::RwLock;
use std::time::Duration;

static SOL_PRICE_USDC: RwLock<f64> = RwLock::new(0.0);

fn init_price(price: f64) {
    if let Ok(mut w) = SOL_PRICE_USDC.write() {
        *w = price;
    }
}

pub fn init() {
    tokio::spawn(async { refresh_loop().await });
}

pub fn sol_price() -> f64 {
    *SOL_PRICE_USDC.read().unwrap_or_else(|e| {
        warn!("SOL price RwLock poisoned, using 0.0");
        // Best-effort recovery: still readable after poison
        e.into_inner()
    })
}

async fn refresh_loop() {
    // Fetch once immediately on startup, override fallback on success
    match fetch_price().await {
        Ok(p) => {
            init_price(p);
            info!("SOL price: ${:.2}", p);
        }
        Err(e) => warn!("initial SOL price fetch failed: {e}"),
    }

    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        match fetch_price().await {
            Ok(p) => {
                init_price(p);
                info!("SOL price: ${:.2}", p);
            }
            Err(e) => warn!("SOL price fetch failed (keeping last price): {e}"),
        }
    }
}

async fn fetch_price() -> anyhow::Result<f64> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap_or_default();

    // Pyth (primary source — Hermes REST API, not blocked by GFW)
    if let Ok(p) = try_source("Pyth", || fetch_pyth(&client)).await {
        return Ok(p);
    }
    // Binance
    if let Ok(p) = try_source("Binance", || fetch_binance(&client)).await {
        return Ok(p);
    }
    // Bybit
    if let Ok(p) = try_source("Bybit", || fetch_bybit(&client)).await {
        return Ok(p);
    }
    // Okx
    if let Ok(p) = try_source("Okx", || fetch_okx(&client)).await {
        return Ok(p);
    }
    // Jupiter
    if let Ok(p) = try_source("Jupiter", || fetch_jupiter(&client)).await {
        return Ok(p);
    }
    // CoinGecko
    if let Ok(p) = try_source("CoinGecko", || fetch_coingecko(&client)).await {
        return Ok(p);
    }

    anyhow::bail!("All price sources failed")
}

/// Single-source retry: wait 1s after failure then retry once
async fn try_source<F, Fut>(name: &str, mut fetch: F) -> anyhow::Result<f64>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<f64>>,
{
    for attempt in 0..2 {
        match fetch().await {
            Ok(p) if p > 0.0 => return Ok(p),
            Ok(p) => debug!("{name}: invalid price {p}"),
            Err(e) => {
                if attempt == 0 {
                    debug!("{name}: {e}, retrying...");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                } else {
                    debug!("{name}: {e} (exhausted)");
                }
            }
        }
    }
    anyhow::bail!("{name}: exhausted")
}

// ============================================================
// Price sources
// ============================================================

/// Binance: SOLUSDC spot price
async fn fetch_binance(client: &reqwest::Client) -> anyhow::Result<f64> {
    let v: serde_json::Value = client
        .get("https://api.binance.com/api/v3/ticker/price?symbol=SOLUSDC")
        .send()
        .await?
        .json()
        .await?;
    v["price"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing price field"))?
        .parse::<f64>()
        .map_err(|e| anyhow::anyhow!("parse: {e}"))
}

/// Bybit: SOLUSDC spot price
async fn fetch_bybit(client: &reqwest::Client) -> anyhow::Result<f64> {
    let v: serde_json::Value = client
        .get("https://api.bybit.com/v5/market/tickers?category=spot&symbol=SOLUSDC")
        .send()
        .await?
        .json()
        .await?;
    v["result"]["list"][0]["lastPrice"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing lastPrice"))?
        .parse::<f64>()
        .map_err(|e| anyhow::anyhow!("parse: {e}"))
}

/// Okx: SOLUSDC spot price
async fn fetch_okx(client: &reqwest::Client) -> anyhow::Result<f64> {
    let v: serde_json::Value = client
        .get("https://www.okx.com/api/v5/market/ticker?instId=SOL-USDC")
        .send()
        .await?
        .json()
        .await?;
    v["data"][0]["last"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing last"))?
        .parse::<f64>()
        .map_err(|e| anyhow::anyhow!("parse: {e}"))
}

/// Jupiter: Solana DEX quote (more accurate but may be blocked by WSL proxy)
async fn fetch_jupiter(client: &reqwest::Client) -> anyhow::Result<f64> {
    let v: serde_json::Value = client
        .get("https://quote-api.jup.ag/v6/price?ids=So11111111111111111111111111111111111111112")
        .send()
        .await?
        .json()
        .await?;
    v["data"]["So11111111111111111111111111111111111111112"]["price"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("unexpected response"))
}

/// CoinGecko: free public API, rate limits are stricter
async fn fetch_coingecko(client: &reqwest::Client) -> anyhow::Result<f64> {
    let v: serde_json::Value = client
        .get("https://api.coingecko.com/api/v3/simple/price?ids=solana&vs_currencies=usd")
        .send()
        .await?
        .json()
        .await?;
    v["solana"]["usd"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("unexpected response"))
}

// ============================================================
// Pyth price (Hermes REST API — not blocked, stable and reliable)
// ============================================================

/// Pyth SOL/USD price feed ID
const PYTH_SOL_USD_FEED: &str =
    "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";

async fn fetch_pyth(client: &reqwest::Client) -> anyhow::Result<f64> {
    let url =
        format!("https://hermes.pyth.network/api/latest_price_feeds?ids[]={PYTH_SOL_USD_FEED}");
    let v: serde_json::Value = client.get(&url).send().await?.json().await?;

    let price_str = v[0]["price"]["price"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Pyth: missing price"))?;
    let expo = v[0]["price"]["expo"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("Pyth: missing expo"))?;

    let raw: f64 = price_str
        .parse()
        .map_err(|e| anyhow::anyhow!("Pyth: parse price: {e}"))?;
    Ok(raw * 10_f64.powi(expo as i32))
}
