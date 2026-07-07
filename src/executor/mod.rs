//! Execution module — single atomic TX submission + background on-chain confirmation
//!
//! Submits directly via RPC sendTransaction without using Jito Block Engine.
//! The target bot also does not need Jito — it wins through high-frequency submission + accepting high failure rate.

pub mod atomic;
pub mod confirmation;
pub mod jito;
pub mod nonce;
pub mod rpc_pool;
pub mod sender;

pub use atomic::{build_atomic_arbitrage_tx, simulate_atomic_tx, submit_atomic_tx};
pub use confirmation::{spawn_confirmation_task, PendingConfirmation};
pub use jito::submit_via_jito;
pub use rpc_pool::RpcPool;
pub use sender::submit_via_sender;
