//! Router assembly.

use axum::routing::{delete, get, post};
use axum::{Router, middleware};

use crate::handlers;
use crate::middleware::require_auth;
use crate::state::AppState;

/// Build the application router.
pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/v1/auth/me", get(handlers::me))
        .route("/api/v1/auth/bootstrap", post(handlers::bootstrap))
        .route("/api/v1/auth/logout", post(handlers::logout))
        .route("/api/v1/auth/mfa/enroll", post(handlers::enroll_mfa))
        .route("/api/v1/auth/sessions", get(handlers::list_sessions))
        .route(
            "/api/v1/auth/sessions/{id}",
            delete(handlers::revoke_session),
        )
        .route("/api/v1/auth/identities", get(handlers::list_identities))
        .route(
            "/api/v1/access-requests",
            get(handlers::list_access_requests),
        )
        .route(
            "/api/v1/access-requests/{user_id}/approve",
            post(handlers::approve_access_request),
        )
        .route("/api/v1/account", delete(handlers::delete_account))
        .route("/api/v1/opportunities", get(handlers::list_opportunities))
        .route(
            "/api/v1/opportunities/{route_id}",
            get(handlers::get_opportunity),
        )
        .route("/api/v1/risk/limits", get(handlers::get_risk_limits))
        .route(
            "/api/v1/positions",
            get(handlers::list_positions).post(handlers::create_position),
        )
        .route("/api/v1/positions/{id}", get(handlers::get_position))
        .route(
            "/api/v1/positions/{id}/close",
            post(handlers::close_position),
        )
        .route("/api/v1/controls", get(handlers::get_controls))
        .route("/api/v1/controls/halt", post(handlers::halt))
        .route(
            "/api/v1/controls/venues/{venue}/disable",
            post(handlers::disable_venue),
        )
        .route("/api/v1/controls/flatten", post(handlers::flatten))
        .route("/api/v1/controls/resume", post(handlers::resume))
        .route(
            "/api/v1/strategies",
            get(handlers::list_strategies).post(handlers::create_strategy),
        )
        .route(
            "/api/v1/strategies/{id}",
            get(handlers::get_strategy)
                .put(handlers::update_strategy)
                .delete(handlers::delete_strategy),
        )
        .route(
            "/api/v1/strategies/{id}/enable",
            post(handlers::enable_strategy),
        )
        .route(
            "/api/v1/strategies/{id}/disable",
            post(handlers::disable_strategy),
        )
        .route(
            "/api/v1/strategies/{id}/apply-to-open",
            post(handlers::apply_strategy_to_open),
        )
        .route("/api/v1/portfolio", get(handlers::get_portfolio))
        .route("/api/v1/health", get(handlers::get_health))
        .route("/api/v1/alerts", get(handlers::list_alerts))
        .route("/api/v1/alerts/{id}/ack", post(handlers::ack_alert))
        .route("/api/v1/venues", get(handlers::list_venues))
        .route("/api/v1/venue-accounts", get(handlers::list_venue_accounts))
        .route(
            "/api/v1/connector-orders",
            get(handlers::list_connector_orders),
        )
        .route(
            "/api/v1/reconciliations",
            get(handlers::list_reconciliations),
        )
        .route(
            "/api/v1/venue-accounts/{id}",
            get(handlers::get_venue_account),
        )
        .route(
            "/api/v1/venue-accounts/{id}/test",
            post(handlers::test_venue_account),
        )
        .route(
            "/api/v1/venue-accounts/{id}/accounting",
            get(handlers::get_connector_accounting),
        )
        .route(
            "/api/v1/venue-accounts/{id}/disable",
            post(handlers::disable_venue_account),
        )
        .route(
            "/api/v1/venue-accounts/{id}/orders/preview",
            post(handlers::preview_connector_order),
        )
        .route(
            "/api/v1/venue-accounts/{id}/orders",
            post(handlers::submit_connector_order),
        )
        .layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let public = Router::new()
        .route("/livez", get(handlers::livez))
        .route("/readyz", get(handlers::readyz))
        .route("/entryz", get(handlers::entryz))
        .route(
            "/api/v1/auth/social/challenges",
            post(handlers::create_challenge),
        )
        .route("/api/v1/auth/social/exchange", post(handlers::exchange))
        .route("/api/v1/auth/refresh", post(handlers::refresh))
        .route("/api/v1/auth/mfa/confirm", post(handlers::confirm_mfa))
        .route("/api/v1/auth/mfa/verify", post(handlers::verify_mfa))
        .route(
            "/api/v1/auth/mfa/recovery-codes/acknowledge",
            post(handlers::acknowledge_recovery),
        );

    public.merge(protected).with_state(state)
}
