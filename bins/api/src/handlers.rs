//! HTTP handlers translating requests into auth-service calls.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use funding_arb_auth_service::AuthError;
use funding_arb_auth_service::dto::{
    AcknowledgeRecoveryRequest, BootstrapRequest, CreateChallengeRequest, ExchangeRequest,
    MfaConfirmRequest, MfaVerifyRequest, RefreshRequest,
};
use funding_arb_auth_service::service::CallerContext;
use funding_arb_scanner_service::OpportunityScanDto;
use serde::Deserialize;

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

/// Optional filters for the opportunity scan feed.
#[derive(Debug, Deserialize)]
pub struct OpportunityQuery {
    /// `SPOT_PERP` or `PERP_PERP` (case-insensitive).
    pub route_type: Option<String>,
    /// A venue that must appear on either leg (case-insensitive).
    pub venue: Option<String>,
    /// Minimum net APR (fraction, e.g. `0.05`).
    pub min_net_apr: Option<f64>,
    /// Minimum profitability probability, `[0, 1]`.
    pub min_probability: Option<f64>,
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
    if let Some(minimum) = filter.min_net_apr {
        scan.opportunities.retain(|opportunity| opportunity.net_apr >= minimum);
    }
    if let Some(minimum) = filter.min_probability {
        scan.opportunities
            .retain(|opportunity| opportunity.profitability_probability >= minimum);
    }

    Ok(Json(scan))
}
