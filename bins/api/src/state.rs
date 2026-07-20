//! Shared application state.

use funding_arb_auth_service::AuthService;
use funding_arb_domain::TradingMode;
use funding_arb_execution_service::{
    BreakersRepo, ExecutionService, MonitoringRepo, OutboxRepo, PositionsRepo,
};
use funding_arb_scanner_service::ScannerRepo;
use funding_arb_strategy_service::StrategiesRepo;
use funding_arb_venue_service::VenueAccountsRepo;

/// Cloneable state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
    /// Fail-closed process trading mode.
    pub mode: TradingMode,
    /// The authentication application service.
    pub service: AuthService,
    /// Read access to the latest funding-arbitrage scan.
    pub scanner: ScannerRepo,
    /// The paper execution service (opens positions from ranked opportunities).
    pub execution: ExecutionService,
    /// Persistence for opened paper positions.
    pub positions: PositionsRepo,
    /// Durable emergency controls.
    pub breakers: BreakersRepo,
    /// Durable domain-event outbox.
    pub outbox: OutboxRepo,
    /// Versioned automated paper strategies.
    pub strategies: StrategiesRepo,
    /// Portfolio, health, and alert read models.
    pub monitoring: MonitoringRepo,
    /// Masked venue account and exact instrument persistence.
    pub venues: VenueAccountsRepo,
}
