//! HTTP handlers translating requests into auth-service calls.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use funding_arb_auth_service::AuthError;
use funding_arb_auth_service::dto::{
    AcknowledgeRecoveryRequest, BootstrapRequest, CreateChallengeRequest, ExchangeRequest,
    MfaConfirmRequest, MfaVerifyRequest, RefreshRequest,
};
use funding_arb_auth_service::service::CallerContext;
use funding_arb_domain::risk::RiskLimits;
use funding_arb_scanner_service::{OpportunityScanDto, RiskLimitsDto};
use serde::Deserialize;
use chrono::{DateTime, Utc};

use crate::problem::ApiError;
use crate::state::AppState;

const BLOCKED_ENTRY: &str =
    r#"{"status":"BLOCKED","reasons":["M0_SCAFFOLD_NO_TRADING_CAPABILITY"]}"#;

/// Liveness probe.
pub async fn livez() -> impl IntoResponse {
    health_ok()
}

/// Readiness probe that verifies database connectivity.
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match state.service.repo().ping().await {
        Ok(()) => health_ok().into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status": "UNAVAILABLE", "mode": "PAPER", "stage": "M1_AUTH"})),
        )
            .into_response(),
    }
}

/// Trading entrypoint probe — still blocked in the auth milestone.
pub async fn entryz() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        BLOCKED_ENTRY,
    )
}

fn health_ok() -> impl IntoResponse {
    Json(serde_json::json!({"status": "OK", "mode": "PAPER", "stage": "M1_AUTH"}))
}

/// `POST /api/v1/auth/social/challenges`
pub async fn create_challenge(
    State(state): State<AppState>,
    Json(request): Json<CreateChallengeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let challenge = state.service.create_challenge(request).await?;
    Ok((StatusCode::CREATED, Json(challenge)))
}

/// `POST /api/v1/auth/social/exchange`
pub async fn exchange(
    State(state): State<AppState>,
    Json(request): Json<ExchangeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(state.service.exchange(request).await?))
}

/// `POST /api/v1/auth/refresh`
pub async fn refresh(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(state.service.refresh(request).await?))
}

/// `GET /api/v1/auth/me`
pub async fn me(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(state.service.me(&caller.user_id).await?))
}

/// `POST /api/v1/auth/bootstrap`
pub async fn bootstrap(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<BootstrapRequest>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(state.service.bootstrap(&caller, request).await?))
}

/// `POST /api/v1/auth/logout`
pub async fn logout(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    state.service.logout(&caller).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/auth/sessions`
pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(state.service.list_sessions(&caller).await?))
}

/// `DELETE /api/v1/auth/sessions/{id}`
pub async fn revoke_session(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    state.service.revoke_session(&caller, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/auth/identities`
pub async fn list_identities(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(state.service.list_identities(&caller).await?))
}

/// `GET /api/v1/access-requests`
pub async fn list_access_requests(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    if !caller.is_owner() {
        return Err(AuthError::Forbidden.into());
    }
    Ok(Json(state.service.list_access_requests().await?))
}

/// `POST /api/v1/access-requests/{user_id}/approve`
pub async fn approve_access_request(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(user_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    if !caller.is_owner() {
        return Err(AuthError::Forbidden.into());
    }
    Ok(Json(state.service.approve_access_request(&user_id).await?))
}

/// `DELETE /api/v1/account`
pub async fn delete_account(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    state.service.delete_account(&caller).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/auth/mfa/enroll`
pub async fn enroll_mfa(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(state.service.enroll_mfa(&caller).await?))
}

/// `POST /api/v1/auth/mfa/confirm`
pub async fn confirm_mfa(
    State(state): State<AppState>,
    Json(request): Json<MfaConfirmRequest>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(state.service.confirm_mfa(request).await?))
}

/// `POST /api/v1/auth/mfa/verify`
pub async fn verify_mfa(
    State(state): State<AppState>,
    Json(request): Json<MfaVerifyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(state.service.verify_mfa(request).await?))
}

/// `POST /api/v1/auth/mfa/recovery-codes/acknowledge`
pub async fn acknowledge_recovery(
    State(state): State<AppState>,
    Json(request): Json<AcknowledgeRecoveryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(Json(state.service.acknowledge_recovery(request).await?))
}

/// Optional filters and sort for the opportunity scan feed (PRODUCT_SPEC §4.6).
#[derive(Debug, Deserialize)]
pub struct OpportunityQuery {
    /// `SPOT_PERP` or `PERP_PERP` (case-insensitive).
    pub route_type: Option<String>,
    /// A venue that must appear on either leg (case-insensitive).
    pub venue: Option<String>,
    /// The underlying asset, e.g. `BTC` (case-insensitive).
    pub underlying: Option<String>,
    /// `POSITIVE` or `NEGATIVE` funding capture (case-insensitive).
    pub funding_direction: Option<String>,
    /// Minimum net APR (fraction, e.g. `0.15`).
    pub min_net_apr: Option<f64>,
    /// Minimum profitability probability, `[0, 1]`.
    pub min_probability: Option<f64>,
    /// Maximum entry basis magnitude, in basis points.
    pub max_basis_bps: Option<f64>,
    /// Maximum seconds until the next funding settlement.
    pub max_time_to_funding_secs: Option<i64>,
    /// Sort: `default`, `net_apr`, `next_funding`, `probability`, `capacity`, `expected_pnl`.
    pub sort: Option<String>,
}

fn seconds_to_funding(next_funding_at: &str, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(next_funding_at)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc).signed_duration_since(now).num_seconds())
}

/// `GET /api/v1/opportunities` — the latest ranked paper scan (read-only).
pub async fn list_opportunities(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Query(filter): Query<OpportunityQuery>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    let mut scan = state
        .scanner
        .latest()
        .await
        .map_err(|error| ApiError(AuthError::Internal(error.to_string())))?
        .unwrap_or(OpportunityScanDto {
            scanned_at: String::new(),
            model_version: String::new(),
            opportunities: Vec::new(),
        });

    if let Some(route_type) = filter.route_type.as_deref() {
        let wanted = route_type.to_ascii_uppercase();
        scan.opportunities.retain(|opportunity| opportunity.route_type == wanted);
    }
    if let Some(venue) = filter.venue.as_deref() {
        let wanted = venue.to_ascii_uppercase();
        scan.opportunities
            .retain(|opportunity| opportunity.long_venue == wanted || opportunity.short_venue == wanted);
    }
    if let Some(underlying) = filter.underlying.as_deref() {
        let wanted = underlying.to_ascii_uppercase();
        scan.opportunities.retain(|opportunity| opportunity.underlying == wanted);
    }
    if let Some(direction) = filter.funding_direction.as_deref() {
        let wanted = direction.to_ascii_uppercase();
        scan.opportunities.retain(|opportunity| opportunity.funding_direction == wanted);
    }
    if let Some(minimum) = filter.min_net_apr {
        scan.opportunities.retain(|opportunity| opportunity.net_apr >= minimum);
    }
    if let Some(minimum) = filter.min_probability {
        scan.opportunities
            .retain(|opportunity| opportunity.profitability_probability >= minimum);
    }
    if let Some(maximum) = filter.max_basis_bps {
        scan.opportunities.retain(|opportunity| opportunity.entry_basis_bps <= maximum);
    }
    if let Some(maximum) = filter.max_time_to_funding_secs {
        let now = Utc::now();
        scan.opportunities.retain(|opportunity| {
            seconds_to_funding(&opportunity.next_funding_at, now).is_none_or(|secs| secs <= maximum)
        });
    }

    match filter.sort.as_deref() {
        Some("net_apr") => scan.opportunities.sort_by(|a, b| b.net_apr.total_cmp(&a.net_apr)),
        Some("next_funding") => scan
            .opportunities
            .sort_by(|a, b| a.next_funding_at.cmp(&b.next_funding_at)),
        Some("probability") => scan
            .opportunities
            .sort_by(|a, b| b.profitability_probability.total_cmp(&a.profitability_probability)),
        Some("capacity") => scan
            .opportunities
            .sort_by(|a, b| b.available_capacity.total_cmp(&a.available_capacity)),
        Some("expected_pnl") => scan
            .opportunities
            .sort_by(|a, b| b.expected_net_pnl.total_cmp(&a.expected_net_pnl)),
        _ => {} // default: persisted rank order
    }

    Ok(Json(scan))
}

/// `GET /api/v1/opportunities/{route_id}` — full detail for one opportunity.
pub async fn get_opportunity(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(route_id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    let found = state
        .scanner
        .opportunity(&route_id)
        .await
        .map_err(|error| ApiError(AuthError::Internal(error.to_string())))?;
    match found {
        Some(opportunity) => Ok(Json(opportunity).into_response()),
        None => Ok((
            StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
            r#"{"type":"about:blank","title":"Opportunity not found","status":404,"code":"OPPORTUNITY_NOT_FOUND"}"#,
        )
            .into_response()),
    }
}

/// `GET /api/v1/risk/limits` — the active §6.2 safe-starter risk limits.
pub async fn get_risk_limits(
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(RiskLimitsDto::from_domain(&RiskLimits::default())))
}
