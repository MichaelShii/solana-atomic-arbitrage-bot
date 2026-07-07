//! Jito Bundle submission directly to Jito Block Engine.
//!
//! Single-TX bundles — tip is injected at build time (see `inject_jito_tip` in atomic/mod.rs).
//! Rate limited to 1 req/s per Jito's free tier.

use anyhow::Context;
use base64::Engine;
use std::sync::atomic::{AtomicI64, Ordering};

static LAST_JITO_SUBMIT_MS: AtomicI64 = AtomicI64::new(0);

/// Submit a transaction directly to Jito Block Engine (Frankfurt).
///
/// Tip must already be baked into the TX at build time.
/// Rate limited to 1 req/s — returns an error if called too fast.
pub async fn submit_via_jito(tx_bytes: &[u8]) -> anyhow::Result<String> {
    // Rate limit: 1 req/s for Jito free tier.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let last = LAST_JITO_SUBMIT_MS.load(Ordering::Relaxed);
    if now_ms - last < 1_100 {
        anyhow::bail!("jito rate limit (1 req/s)");
    }
    LAST_JITO_SUBMIT_MS.store(now_ms, Ordering::Relaxed);

    let base64_tx = base64::engine::general_purpose::STANDARD.encode(tx_bytes);

    // Try sendTransaction first (direct to Jito validators, no auction).
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendTransaction",
        "params": [base64_tx, {"encoding": "base64", "skipPreflight": true, "maxRetries": 0}]
    });

    let jito_url = "https://frankfurt.mainnet.block-engine.jito.wtf/api/v1/transactions";

    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build reqwest client")?
        .post(jito_url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .context("jito HTTP POST")?;

    let status = resp.status();
    let text = resp.text().await.context("read jito response")?;

    if !status.is_success() {
        anyhow::bail!("jito returned {status}: {text}");
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).context("parse jito response")?;

    if let Some(err) = json.get("error") {
        anyhow::bail!("jito RPC error: {err}");
    }

    json.get("result")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
        .context("jito response missing 'result' field")
}
