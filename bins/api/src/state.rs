//! Shared application state.

use funding_arb_auth_service::{AuthService, PasskeyService};
use funding_arb_domain::TradingMode;
use funding_arb_execution_service::{
    BreakersRepo, ExecutionService, MonitoringRepo, OutboxRepo, PositionsRepo,
};
use funding_arb_scanner_service::ScannerRepo;
use funding_arb_strategy_service::StrategiesRepo;
use funding_arb_venue_service::{
    ConnectorExecutionCoordinator, ConnectorRegistry, ReconciliationService,
};
use funding_arb_venue_service::{
    ConnectorMonitoringRepo, CredentialEnrollmentService, VenueAccountsRepo,
};
use std::sync::Arc;

/// Cloneable state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
    /// Fail-closed process trading mode.
    pub mode: TradingMode,
    /// The authentication application service.
    pub service: AuthService,
    /// WebAuthn passkey service; `None` when passkeys are not configured.
    pub passkeys: Option<PasskeyService>,
    /// Read access to the latest funding-arbitrage scan.
    pub scanner: ScannerRepo,
    /// The paper execution service (opens positions from ranked opportunities).
    pub execution: ExecutionService,
    /// Oldest persisted market snapshot a paper open may use (DEC-0016).
    pub max_snapshot_age: chrono::Duration,
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
    /// Exact connector orders, reconciliations, balances, and venue positions.
    pub connector_monitoring: ConnectorMonitoringRepo,
    /// Encrypted connector registry; absent in paper mode.
    pub connector_registry: Option<Arc<ConnectorRegistry>>,
    /// Passkey-gated credential enrollment; absent when no secret backend is
    /// configured (paper/dev), so live secrets are never accepted there.
    pub enrollment: Option<Arc<CredentialEnrollmentService>>,
    /// Durable sandbox submit-once coordinator.
    pub connector_execution: ConnectorExecutionCoordinator,
    /// Read-only venue reconciliation used by credential preflight.
    pub reconciliation: ReconciliationService,
}
