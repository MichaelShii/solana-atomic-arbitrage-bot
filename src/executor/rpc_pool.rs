//! Multi-RPC pool with round-robin load distribution and failover.
//!
//! H-03: wraps a primary RPC client + fallback endpoints. Each call to
//! `current()` round-robins to the next endpoint, distributing load across
//! all providers to stay under rate limits. On connection failure,
//! `rotate()` skips the failing endpoint and advances to the next.

use solana_client::nonblocking::rpc_client::RpcClient;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Pool of RPC clients with round-robin load distribution.
pub struct RpcPool {
    clients: Vec<Arc<RpcClient>>,
    counter: AtomicUsize,
}

impl RpcPool {
    /// Build a pool from a primary URL + optional fallback URLs.
    pub fn new(
        primary_url: &str,
        fallback_urls: &[String],
        timeout: Duration,
    ) -> Self {
        let mut urls = vec![primary_url.to_string()];
        urls.extend(fallback_urls.iter().cloned());

        let clients: Vec<Arc<RpcClient>> = urls
            .iter()
            .map(|url| Arc::new(RpcClient::new_with_timeout(url.clone(), timeout)))
            .collect();

        log::info!(
            "[RPC POOL] {} endpoint(s): primary={}, fallbacks={}",
            clients.len(),
            &primary_url[..primary_url.len().min(60)],
            fallback_urls.len(),
        );

        RpcPool {
            clients,
            counter: AtomicUsize::new(0),
        }
    }

    /// Get the next RPC client in round-robin order.
    /// Distributes load across all endpoints to avoid rate limits.
    pub fn current(&self) -> Arc<RpcClient> {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % self.clients.len();
        self.clients[idx].clone()
    }

    /// Skip the current endpoint on failure and return the next one.
    /// Advances the counter so subsequent calls avoid the failing endpoint.
    pub fn rotate(&self) -> Option<Arc<RpcClient>> {
        // Advance by an extra step to skip the failing endpoint
        let idx = self.counter.fetch_add(2, Ordering::Relaxed) % self.clients.len();
        log::warn!(
            "[RPC POOL] skipping endpoint → {} ({} total)",
            idx, self.clients.len(),
        );
        Some(self.clients[idx].clone())
    }

    /// Health check an endpoint by calling get_slot.
    pub async fn health_check(&self) -> bool {
        let client = self.current();
        match tokio::time::timeout(Duration::from_secs(5), client.get_slot()).await {
            Ok(Ok(_)) => true,
            _ => false,
        }
    }

    /// Number of endpoints in the pool.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.clients.len()
    }
}
