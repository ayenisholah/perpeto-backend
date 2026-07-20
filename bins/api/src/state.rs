//! Shared application state.

use funding_arb_auth_service::AuthService;
use funding_arb_execution_service::{BreakersRepo, ExecutionService, OutboxRepo, PositionsRepo};
use funding_arb_scanner_service::ScannerRepo;

/// Cloneable state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
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
}
