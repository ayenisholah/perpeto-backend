//! Shared application state.

use funding_arb_auth_service::AuthService;
use funding_arb_scanner_service::ScannerRepo;

/// Cloneable state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
    /// The authentication application service.
    pub service: AuthService,
    /// Read access to the latest funding-arbitrage scan.
    pub scanner: ScannerRepo,
}
