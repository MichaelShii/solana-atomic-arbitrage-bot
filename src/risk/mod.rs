//! Runtime risk control module (Phase 5)
//!
//! Adds runtime state tracking on top of config-level RiskConfig:
//! - Daily PnL accumulation
//! - Circuit breaker (stop trading after daily loss limit triggered)
//! - Auto-reset across days

use chrono::{NaiveDate, Utc};
use log::{info, warn};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use crate::arbitrage::ArbitrageOpportunity;
use crate::config::RiskConfig;

/// Circuit breaker auto-reset cooldown (30 minutes)
const BREAKER_COOLDOWN_SECS: u64 = 1800;

// ============================================================
// Types
// ============================================================

pub struct RiskTracker {
    pub current_date: NaiveDate,
    pub daily_pnl_sol: f64,
    pub trades_attempted: u64,
    pub trades_pending: u64,
    pub trades_succeeded: u64,
    pub trades_failed_onchain: u64,
    pub total_net_profit_sol: f64,
    /// Cumulative priority + base fee wasted from on-chain failures (SOL)
    pub cumulative_failed_fee_sol: f64,
    pub circuit_breaker: bool,
    pub breaker_reason: Option<String>,
    pub breaker_triggered_at: Option<Instant>,
}

pub type SharedRiskTracker = Arc<Mutex<RiskTracker>>;

// ============================================================
// Impl
// ============================================================

impl RiskTracker {
    pub fn new() -> Self {
        Self {
            current_date: Utc::now().date_naive(),
            daily_pnl_sol: 0.0,
            trades_attempted: 0,
            trades_pending: 0,
            trades_succeeded: 0,
            trades_failed_onchain: 0,
            total_net_profit_sol: 0.0,
            cumulative_failed_fee_sol: 0.0,
            circuit_breaker: false,
            breaker_reason: None,
            breaker_triggered_at: None,
        }
    }

    /// Check if a single arbitrage opportunity passes all risk control limits
    pub fn check_limits(&mut self, opp: &ArbitrageOpportunity, risk: &RiskConfig) -> bool {
        if self.circuit_breaker {
            // Auto-reset if cooldown has elapsed
            if let Some(triggered) = self.breaker_triggered_at {
                if triggered.elapsed().as_secs() >= BREAKER_COOLDOWN_SECS {
                    info!(
                        "[RISK] event=breaker_cooldown_elapsed reason={:?}",
                        self.breaker_reason
                    );
                    self.circuit_breaker = false;
                    self.breaker_reason = None;
                    self.breaker_triggered_at = None;
                    self.daily_pnl_sol = 0.0;
                } else {
                    return false;
                }
            } else {
                return false;
            }
        }

        let today = Utc::now().date_naive();
        if today != self.current_date {
            self.current_date = today;
            self.daily_pnl_sol = 0.0;
            self.trades_attempted = 0;
            self.trades_pending = 0;
            self.trades_succeeded = 0;
            self.trades_failed_onchain = 0;
            self.cumulative_failed_fee_sol = 0.0;
        }

        if opp.net_profit_sol < risk.min_profit_threshold_sol {
            return false;
        }

        if opp.investment_sol > risk.max_single_investment_sol {
            return false;
        }

        if self.daily_pnl_sol + opp.net_profit_sol < -risk.max_daily_loss_sol {
            self.circuit_breaker = true;
            self.breaker_triggered_at = Some(Instant::now());
            self.breaker_reason = Some(format!(
                "daily loss limit exceeded: {:.3} SOL",
                risk.max_daily_loss_sol
            ));
            warn!(
                "[RISK] event=circuit_breaker daily_pnl={:.3} projected={:.3} limit=-{:.3} cooldown={}s",
                self.daily_pnl_sol,
                opp.net_profit_sol,
                risk.max_daily_loss_sol,
                BREAKER_COOLDOWN_SECS,
            );
            return false;
        }

        true
    }

    /// RPC has accepted the transaction, waiting for on-chain confirmation. Does not count PnL, success/failure.
    pub fn record_submitted(&mut self) {
        self.trades_attempted += 1;
        self.trades_pending += 1;
    }

    /// Callback after on-chain confirmation: decrement pending, update counters and correct PnL by actual result
    /// actual_pnl_sol = post_balance - pre_balance (includes fees, can be negative)
    pub fn record_confirmation(&mut self, actual_pnl_sol: f64, succeeded: bool, risk: &RiskConfig) {
        self.trades_pending = self.trades_pending.saturating_sub(1);
        if succeeded {
            self.trades_succeeded += 1;
        } else {
            self.trades_failed_onchain += 1;
            self.cumulative_failed_fee_sol += (-actual_pnl_sol).max(0.0);
        }
        self.daily_pnl_sol += actual_pnl_sol;
        self.total_net_profit_sol += actual_pnl_sol;

        self.check_circuit_breaker(risk);
    }

    fn check_circuit_breaker(&mut self, risk: &RiskConfig) {
        if self.daily_pnl_sol < -risk.max_daily_loss_sol {
            self.circuit_breaker = true;
            self.breaker_triggered_at = Some(Instant::now());
            self.breaker_reason = Some(format!(
                "daily loss limit exceeded after trade: daily_pnl={:.3} < -{:.3}",
                self.daily_pnl_sol, risk.max_daily_loss_sol
            ));
            warn!("[RISK] event=circuit_breaker_after_trade daily_pnl={:.3} limit=-{:.3} cooldown={}s",
                self.daily_pnl_sol, risk.max_daily_loss_sol, BREAKER_COOLDOWN_SECS);
        }
    }

    #[allow(dead_code)] // public API for external circuit breaker monitoring
    pub fn is_blocked(&self) -> bool {
        self.circuit_breaker
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arbitrage::{ArbitrageOpportunity, Venue};

    fn test_risk_config() -> RiskConfig {
        crate::config::RiskConfig {
            min_profit_threshold_sol: 0.001,
            max_single_investment_sol: 50.0,
            max_daily_loss_sol: 10.0,
            slippage_tolerance_bps: 100,
            max_tip_sol: 0.5,
            blacklist: crate::config::BlacklistConfig {
                tokens: vec![],
                wallets: vec![],
                programs: vec![],
            },
        }
    }

    fn test_opportunity(investment_sol: f64, net_profit_sol: f64) -> ArbitrageOpportunity {
        ArbitrageOpportunity {
            signature: "test_sig".into(),
            slot: 300_000_000,
            token_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
            buy_venue: Venue::PumpSwapAmm,
            sell_venue: Venue::MeteoraDlmm,
            buy_price_sol: 0.001,
            sell_price_sol: 0.0015,
            price_diff_bps: 500,
            investment_sol,
            expected_profit_sol: net_profit_sol + 0.001,
            net_profit_sol,
            confidence: 0.9,
            dlmm_fee_bps: 25,
        }
    }

    #[test]
    fn accepts_valid_opportunity() {
        let config = test_risk_config();
        let mut tracker = RiskTracker::new();
        let opp = test_opportunity(10.0, 0.05);
        assert!(tracker.check_limits(&opp, &config));
    }

    #[test]
    fn circuit_breaker_stops_trading() {
        let config = test_risk_config();
        let mut tracker = RiskTracker::new();
        tracker.circuit_breaker = true;
        tracker.breaker_reason = Some("test breaker".into());
        tracker.breaker_triggered_at = Some(Instant::now());
        let opp = test_opportunity(10.0, 0.05);
        assert!(!tracker.check_limits(&opp, &config));
    }

    #[test]
    fn rejects_over_max_investment() {
        let mut config = test_risk_config();
        config.max_single_investment_sol = 1.0;
        let mut tracker = RiskTracker::new();
        let opp = test_opportunity(50.0, 0.05);
        assert!(!tracker.check_limits(&opp, &config));
    }

    #[test]
    fn daily_pnl_reset_on_new_day() {
        let mut tracker = RiskTracker::new();
        tracker.daily_pnl_sol = -50.0;
        tracker.current_date = chrono::Utc::now().date_naive() - chrono::Duration::days(1);

        let config = test_risk_config();
        let opp = test_opportunity(10.0, 0.05);
        let result = tracker.check_limits(&opp, &config);
        assert_eq!(tracker.daily_pnl_sol, 0.0);
        assert!(result);
    }

    #[test]
    fn record_submitted_increments_pending() {
        let mut tracker = RiskTracker::new();
        tracker.record_submitted();
        assert_eq!(tracker.trades_attempted, 1);
        assert_eq!(tracker.trades_pending, 1);
        assert_eq!(tracker.trades_succeeded, 0);
        assert_eq!(tracker.daily_pnl_sol, 0.0);
    }

    #[test]
    fn record_confirmation_success_updates_counters() {
        let mut tracker = RiskTracker::new();
        let config = test_risk_config();
        tracker.record_submitted();
        tracker.record_confirmation(0.3, true, &config);
        assert_eq!(tracker.trades_pending, 0);
        assert_eq!(tracker.trades_succeeded, 1);
        assert_eq!(tracker.trades_failed_onchain, 0);
        assert!((tracker.daily_pnl_sol - 0.3).abs() < 1e-9);
        assert!((tracker.total_net_profit_sol - 0.3).abs() < 1e-9);
    }

    #[test]
    fn record_confirmation_failure_updates_counters() {
        let mut tracker = RiskTracker::new();
        let config = test_risk_config();
        tracker.record_submitted();
        tracker.record_confirmation(-0.005, false, &config);
        assert_eq!(tracker.trades_pending, 0);
        assert_eq!(tracker.trades_succeeded, 0);
        assert_eq!(tracker.trades_failed_onchain, 1);
        assert!((tracker.cumulative_failed_fee_sol - 0.005).abs() < 1e-9);
        assert!((tracker.daily_pnl_sol - (-0.005)).abs() < 1e-9);
    }
}
