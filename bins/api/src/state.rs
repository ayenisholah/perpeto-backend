//! Shared application state.

use funding_arb_auth_service::AuthService;

/// Cloneable state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
    /// The authentication application service.
    pub service: AuthService,
}
