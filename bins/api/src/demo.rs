//! The read-only public demo (DEC-0017).
//!
//! `PERPETO_DEMO_ENABLED=true` (PAPER only) seeds one demo tenant whose single
//! principal holds only `VIEWER` and is flagged `demo`, plus one enabled paper
//! strategy so the engine trades it and the read models have something to show.
//! Visitors receive short-lived, access-only sessions from
//! `POST /api/v1/demo/session`; `require_auth` refuses every non-read request
//! they make.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use funding_arb_auth_service::AuthService;
use funding_arb_strategy_service::{
    AllocationDto, AutoCompoundDto, StrategiesRepo, StrategyDto, StrategyVersionDto,
};
use funding_arb_tenant_context::{TenantContext, TenantId};
use rust_decimal::Decimal;
use uuid::Uuid;

/// Demo sessions the whole deployment issues per minute. Each lasts as long as
/// its access token, so this bounds session rows without tracking visitors.
const SESSIONS_PER_MINUTE: u32 = 120;

/// Admission control for anonymous demo sessions.
#[derive(Debug)]
pub struct DemoGate {
    window: Mutex<(Instant, u32)>,
}

impl DemoGate {
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: Mutex::new((Instant::now(), 0)),
        }
    }

    /// Whether another session may be issued in the current one-minute window.
    pub fn admit(&self) -> bool {
        let Ok(mut window) = self.window.lock() else {
            return false;
        };
        if window.0.elapsed() >= Duration::from_secs(60) {
            *window = (Instant::now(), 0);
        }
        if window.1 >= SESSIONS_PER_MINUTE {
            return false;
        }
        window.1 += 1;
        true
    }
}

impl Default for DemoGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Create the demo principal and, when its tenant has no strategy yet, one
/// enabled paper strategy. Idempotent across restarts and replicas.
///
/// # Errors
/// Returns a description of the failed step.
pub async fn seed(service: &AuthService, strategies: &StrategiesRepo) -> Result<(), String> {
    let principal = service
        .ensure_demo_principal()
        .await
        .map_err(|error| format!("demo principal: {error}"))?;
    let tenant = principal
        .tenant_id
        .parse::<TenantId>()
        .map(TenantContext::new)
        .map_err(|_| "demo tenant id is not a UUID".to_owned())?;
    let existing = strategies
        .list(tenant)
        .await
        .map_err(|error| format!("demo strategies: {error}"))?;
    if existing.is_empty() {
        strategies
            .create(tenant, &demo_strategy(&principal.user_id))
            .await
            .map_err(|error| format!("demo strategy: {error}"))?;
    }
    Ok(())
}

/// The demo tenant's paper strategy: both route types across the three
/// public venues, a fixed 1,000 USDT allocation and conservative exits.
#[must_use]
pub fn demo_strategy(creator: &str) -> StrategyDto {
    StrategyDto {
        id: Uuid::new_v4().to_string(),
        name: "Demo: BTC and ETH funding carry".to_owned(),
        enabled: true,
        current: StrategyVersionDto {
            version: 1,
            creator: creator.to_owned(),
            created_at: Utc::now().to_rfc3339(),
            mode: "PAPER".to_owned(),
            route_types: vec!["SPOT_PERP".to_owned(), "PERP_PERP".to_owned()],
            allowed_venues: vec!["BINANCE".to_owned(), "BYBIT".to_owned(), "OKX".to_owned()],
            allowed_assets: vec!["BTC".to_owned(), "ETH".to_owned()],
            min_net_apr: Decimal::new(5, 2),
            min_profitability: Decimal::new(55, 2),
            min_forecast_coverage: Decimal::new(5, 1),
            max_time_to_funding_minutes: 480,
            max_basis_bps: Decimal::new(25, 0),
            allocation: AllocationDto {
                method: "FIXED".to_owned(),
                value: Some(Decimal::new(1_000, 0)),
                minimum: None,
                maximum: None,
            },
            max_concurrent: 2,
            cooldown_seconds: 1_800,
            hold_seconds: 28_800,
            auto_compound: AutoCompoundDto {
                enabled: false,
                reinvest_fraction: Decimal::ZERO,
                minimum_reinvest_amount: Decimal::ZERO,
            },
            exit_net_apr: Decimal::new(1, 2),
            take_profit_fraction: None,
            stop_loss_fraction: Some(Decimal::new(2, 2)),
            max_drawdown_fraction: Decimal::new(5, 2),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_strategy_is_a_valid_paper_strategy() {
        let strategy = demo_strategy(&Uuid::new_v4().to_string());
        let domain = strategy.to_domain().expect("valid strategy");
        assert!(domain.enabled);
        assert_eq!(strategy.current.mode, "PAPER");
    }

    #[test]
    fn the_gate_admits_a_bounded_number_of_sessions_per_minute() {
        let gate = DemoGate::new();
        for _ in 0..SESSIONS_PER_MINUTE {
            assert!(gate.admit());
        }
        assert!(!gate.admit());
    }
}
