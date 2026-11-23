//! HTTP handlers translating requests into application-service calls.

use std::cmp::Reverse;
use std::str::FromStr;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use funding_arb_auth_service::AuthError;
use funding_arb_auth_service::dto::{
    AcknowledgeRecoveryRequest, BootstrapRequest, CreateChallengeRequest, ExchangeRequest,
    MfaConfirmRequest, MfaVerifyRequest, RefreshRequest,
};
use funding_arb_auth_service::service::CallerContext;
use funding_arb_connectors::{
    AccountId, CanonicalOrderIntent, InstrumentId, OrderSide, ProductType,
};
use funding_arb_domain::auth::ActionDescriptor;
use funding_arb_domain::market::Venue;
use funding_arb_domain::pilot::{LiveRiskDenied, PilotError, PilotState, permit_live_risk};
use funding_arb_domain::risk::RiskLimits;
use funding_arb_execution_service::{
    BreakerAction, BreakerScope, OpenError, OpenPositionRequest, PositionDto, PositionListDto,
};
use funding_arb_scanner_service::{OpportunityScanDto, RiskLimitsDto};
use funding_arb_strategy_service::StrategyDto;
use funding_arb_venue_service::{CertificationState, ConnectionTestDto, VenueSummaryDto};
use funding_arb_venue_service::{ConfiguredConnector, SubmissionState};
use funding_arb_venue_service::{
    CredentialBundle, CredentialEnrollmentService, EnrollRequest, EnrollmentError, PreflightError,
    PreflightService, SecretStoreError, VenueAccountDto,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::problem::ApiError;
use crate::state::AppState;

/// Liveness probe.
pub async fn livez(State(state): State<AppState>) -> impl IntoResponse {
    health_ok(state.mode)
}

/// Readiness probe that verifies database connectivity.
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match state.service.repo().ping().await {
        Ok(()) => health_ok(state.mode).into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status": "UNAVAILABLE", "mode": state.mode.to_string(), "stage": "M3_CONNECTORS"})),
        )
            .into_response(),
    }
}

/// Trading entrypoint probe — still blocked in the auth milestone.
pub async fn entryz(State(state): State<AppState>) -> impl IntoResponse {
    let ready = state.mode.allows_external_writes();
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(serde_json::json!({
            "status": if ready { "READY" } else { "BLOCKED" },
            "mode": state.mode.to_string(),
            "external_writes": ready,
            "reasons": if ready { Vec::<String>::new() } else { vec![String::from("MODE_FORBIDS_EXTERNAL_WRITES")] },
        })),
    )
}

fn health_ok(mode: funding_arb_domain::TradingMode) -> impl IntoResponse {
    Json(serde_json::json!({"status": "OK", "mode": mode.to_string(), "stage": "M3_CONNECTORS"}))
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

/// `GET /api/v1/audit`
///
/// Owner-only tamper-evident activity log for the caller's tenant: the
/// hash-chained control-plane events plus an `intact` integrity flag. Exposes
/// only pseudonymized actors and non-secret details.
pub async fn get_audit_log(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    if !caller.is_owner() {
        return Err(AuthError::Forbidden.into());
    }
    Ok(Json(state.service.audit_log(&caller).await?))
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

/// Body for finishing a passkey registration ceremony.
#[derive(Debug, Deserialize)]
pub struct FinishRegistrationRequest {
    pub challenge_id: String,
    pub label: String,
    /// Opaque `RegisterPublicKeyCredential` produced by the authenticator.
    pub credential: serde_json::Value,
}

/// Body for finishing a passkey assertion ceremony.
#[derive(Debug, Deserialize)]
pub struct FinishAuthenticationRequest {
    pub challenge_id: String,
    /// Opaque `PublicKeyCredential` produced by the authenticator.
    pub credential: serde_json::Value,
}

/// Resolve the configured passkey service or fail with 503.
fn passkeys(state: &AppState) -> Result<&funding_arb_auth_service::PasskeyService, ApiError> {
    state
        .passkeys
        .as_ref()
        .ok_or_else(|| AuthError::PasskeysUnavailable.into())
}

/// `POST /api/v1/passkeys/register/start`
pub async fn start_passkey_registration(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(passkeys(&state)?.start_registration(&caller).await?))
}

/// `POST /api/v1/passkeys/register/finish`
pub async fn finish_passkey_registration(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<FinishRegistrationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    let view = passkeys(&state)?
        .finish_registration(
            &caller,
            &request.challenge_id,
            &request.label,
            request.credential,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(view)))
}

/// `POST /api/v1/passkeys/authenticate/start`
pub async fn start_passkey_authentication(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(passkeys(&state)?.start_authentication(&caller).await?))
}

/// `POST /api/v1/passkeys/authenticate/finish`
pub async fn finish_passkey_authentication(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<FinishAuthenticationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        passkeys(&state)?
            .finish_authentication(&caller, &request.challenge_id, request.credential)
            .await?,
    ))
}

/// `GET /api/v1/passkeys`
pub async fn list_passkeys(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(passkeys(&state)?.list(&caller).await?))
}

/// `POST /api/v1/passkeys/{id}/revoke`
pub async fn revoke_passkey(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    passkeys(&state)?.revoke(&caller, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Credential lifecycle (passkey-gated) ---------------------------------

/// Write-only exchange credential material submitted during enroll/rotate.
/// Deliberately not `Debug` so it cannot be logged.
#[derive(Deserialize)]
pub struct CredentialInput {
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
}

impl CredentialInput {
    fn into_bundle(self) -> CredentialBundle {
        CredentialBundle {
            api_key: self.api_key,
            api_secret: self.api_secret,
            passphrase: self.passphrase,
        }
    }
}

/// `{venue,alias,product,environment}` for a new-account enrollment.
#[derive(Debug, Deserialize)]
pub struct EnrollStartRequest {
    pub venue: String,
    pub alias: String,
    pub product: String,
    pub environment: String,
}

#[derive(Deserialize)]
pub struct EnrollFinishRequest {
    pub challenge_id: String,
    pub venue: String,
    pub alias: String,
    pub product: String,
    pub environment: String,
    /// Opaque WebAuthn assertion authorizing the enrollment.
    pub assertion: serde_json::Value,
    pub credential: CredentialInput,
}

/// `{product}` plus the account id from the path, for rotate/revoke start.
#[derive(Debug, Deserialize)]
pub struct ProductRequest {
    pub product: String,
}

#[derive(Deserialize)]
pub struct RotateFinishRequest {
    pub challenge_id: String,
    pub product: String,
    pub assertion: serde_json::Value,
    pub credential: CredentialInput,
}

#[derive(Debug, Deserialize)]
pub struct RevokeFinishRequest {
    pub challenge_id: String,
    pub product: String,
    pub assertion: serde_json::Value,
}

/// Account-level deletion binds no product, so it carries only the proof.
#[derive(Debug, Deserialize)]
pub struct DeleteFinishRequest {
    pub challenge_id: String,
    pub assertion: serde_json::Value,
}

/// Resolve the credential enrollment service or fail with 503.
fn require_enrollment(state: &AppState) -> Result<&CredentialEnrollmentService, ApiError> {
    state
        .enrollment
        .as_deref()
        .ok_or_else(|| AuthError::CredentialsUnavailable.into())
}

/// Map a redacted [`EnrollmentError`] to a transport error. Vault unavailability
/// surfaces as 503; nothing here echoes credential material.
fn map_enrollment(error: EnrollmentError) -> ApiError {
    match error {
        EnrollmentError::InvalidRequest => AuthError::BadRequest("invalid enrollment request"),
        EnrollmentError::NotFound => AuthError::NotFound,
        EnrollmentError::Conflict => AuthError::Conflict("venue account already exists"),
        EnrollmentError::InvalidState => AuthError::Conflict("invalid connection state"),
        EnrollmentError::Secret(SecretStoreError::VaultUnavailable) => {
            AuthError::CredentialsUnavailable
        }
        EnrollmentError::Secret(_) | EnrollmentError::Persistence => {
            AuthError::Internal("credential operation failed".to_owned())
        }
    }
    .into()
}

/// Map a [`PreflightError`] to a transport error. Registry/persistence failures
/// surface as internal errors; reconciliation *failure* is not an error here (it
/// records `PREFLIGHT_FAILED`), so it never reaches this mapping.
fn map_preflight(error: PreflightError) -> ApiError {
    match error {
        PreflightError::NotFound => AuthError::NotFound.into(),
        PreflightError::InvalidProduct => AuthError::BadRequest("invalid connector product").into(),
        PreflightError::Registry(error) => ApiError(AuthError::Internal(error.to_string())),
        PreflightError::Enrollment(error) => map_enrollment(error),
    }
}

/// Require an active trader/owner for credential mutations.
fn require_credential_role(caller: &CallerContext) -> Result<(), ApiError> {
    caller.require_active()?;
    if caller.can_trade() {
        Ok(())
    } else {
        Err(AuthError::Forbidden.into())
    }
}

/// `POST /api/v1/venue-credentials/enroll/start`
pub async fn enroll_credential_start(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<EnrollStartRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_credential_role(&caller)?;
    require_enrollment(&state)?;
    let action = ActionDescriptor::EnrollCredential {
        venue: request.venue,
        alias: request.alias,
        product: request.product,
        environment: request.environment,
    };
    Ok(Json(
        passkeys(&state)?.start_action(&caller, &action).await?,
    ))
}

/// `POST /api/v1/venue-credentials/enroll/finish`
pub async fn enroll_credential_finish(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<EnrollFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_credential_role(&caller)?;
    let enrollment = require_enrollment(&state)?;
    let action = ActionDescriptor::EnrollCredential {
        venue: request.venue.clone(),
        alias: request.alias.clone(),
        product: request.product.clone(),
        environment: request.environment.clone(),
    };
    // Consume the one-use passkey proof before touching any secret.
    passkeys(&state)?
        .finish_action(&caller, &request.challenge_id, &action, request.assertion)
        .await?;
    let enroll_request = EnrollRequest {
        venue: request.venue,
        alias: request.alias,
        product: request.product,
        environment: request.environment,
    };
    let tenant = caller.tenant_context()?;
    let account = enrollment
        .enroll(tenant, &enroll_request, &request.credential.into_bundle())
        .await
        .map_err(map_enrollment)?;
    audit_action(
        &state,
        &caller,
        "CREDENTIAL_ENROLLED",
        "venue_account",
        &account.id,
        serde_json::json!({ "venue": account.venue.clone(), "product": enroll_request.product }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(account)))
}

/// `POST /api/v1/venue-credentials/{id}/rotate/start`
pub async fn rotate_credential_start(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<ProductRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_credential_role(&caller)?;
    require_enrollment(&state)?;
    let account = require_enrollment_account(&state, &caller, &id).await?;
    let action = ActionDescriptor::RotateCredential {
        venue_account_id: id,
        venue: account.venue,
        product: request.product,
    };
    Ok(Json(
        passkeys(&state)?.start_action(&caller, &action).await?,
    ))
}

/// `POST /api/v1/venue-credentials/{id}/rotate/finish`
pub async fn rotate_credential_finish(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<RotateFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_credential_role(&caller)?;
    let enrollment = require_enrollment(&state)?;
    let account = require_enrollment_account(&state, &caller, &id).await?;
    let action = ActionDescriptor::RotateCredential {
        venue_account_id: id.clone(),
        venue: account.venue,
        product: request.product.clone(),
    };
    passkeys(&state)?
        .finish_action(&caller, &request.challenge_id, &action, request.assertion)
        .await?;
    let tenant = caller.tenant_context()?;
    let updated = enrollment
        .rotate(
            tenant,
            &id,
            &request.product,
            &request.credential.into_bundle(),
        )
        .await
        .map_err(map_enrollment)?;
    audit_action(
        &state,
        &caller,
        "CREDENTIAL_ROTATED",
        "venue_account",
        &id,
        serde_json::json!({ "product": request.product }),
    )
    .await;
    Ok(Json(updated))
}

/// `POST /api/v1/venue-credentials/{id}/revoke/start`
pub async fn revoke_credential_start(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<ProductRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_credential_role(&caller)?;
    require_enrollment(&state)?;
    let account = require_enrollment_account(&state, &caller, &id).await?;
    let action = ActionDescriptor::RevokeCredential {
        venue_account_id: id,
        venue: account.venue,
        product: request.product,
    };
    Ok(Json(
        passkeys(&state)?.start_action(&caller, &action).await?,
    ))
}

/// `POST /api/v1/venue-credentials/{id}/revoke/finish`
pub async fn revoke_credential_finish(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<RevokeFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_credential_role(&caller)?;
    let enrollment = require_enrollment(&state)?;
    let account = require_enrollment_account(&state, &caller, &id).await?;
    let action = ActionDescriptor::RevokeCredential {
        venue_account_id: id.clone(),
        venue: account.venue,
        product: request.product.clone(),
    };
    passkeys(&state)?
        .finish_action(&caller, &request.challenge_id, &action, request.assertion)
        .await?;
    let tenant = caller.tenant_context()?;
    let updated = enrollment
        .revoke(tenant, &id, &request.product)
        .await
        .map_err(map_enrollment)?;
    audit_action(
        &state,
        &caller,
        "CREDENTIAL_REVOKED",
        "venue_account",
        &id,
        serde_json::json!({ "product": request.product }),
    )
    .await;
    Ok(Json(updated))
}

/// `POST /api/v1/venue-credentials/{id}/delete/start`
pub async fn delete_credential_start(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    require_credential_role(&caller)?;
    require_enrollment(&state)?;
    let account = require_enrollment_account(&state, &caller, &id).await?;
    let action = ActionDescriptor::DeleteCredential {
        venue_account_id: id,
        venue: account.venue,
    };
    Ok(Json(
        passkeys(&state)?.start_action(&caller, &action).await?,
    ))
}

/// `POST /api/v1/venue-credentials/{id}/delete/finish`
///
/// Destroys every connection secret and tombstones the account. Refused with 409
/// while a pilot is armed (disarm — which flattens — first); 204 on success.
pub async fn delete_credential_finish(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<DeleteFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_credential_role(&caller)?;
    let enrollment = require_enrollment(&state)?;
    let account = require_enrollment_account(&state, &caller, &id).await?;
    if state.service.pilot_state(&caller).await? == PilotState::PilotArmed {
        return Err(AuthError::Conflict("disarm the pilot before deleting its credential").into());
    }
    let action = ActionDescriptor::DeleteCredential {
        venue_account_id: id.clone(),
        venue: account.venue,
    };
    passkeys(&state)?
        .finish_action(&caller, &request.challenge_id, &action, request.assertion)
        .await?;
    let tenant = caller.tenant_context()?;
    enrollment
        .delete(tenant, &id)
        .await
        .map_err(map_enrollment)?;
    audit_action(
        &state,
        &caller,
        "CREDENTIAL_DELETED",
        "venue_account",
        &id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Load the caller's venue account (tenant-scoped) so an action can be bound to
/// its exact venue; 404 if it does not exist.
async fn require_enrollment_account(
    state: &AppState,
    caller: &CallerContext,
    id: &str,
) -> Result<VenueAccountDto, ApiError> {
    let tenant = caller.tenant_context()?;
    state
        .venues
        .get(tenant, id)
        .await
        .map_err(|_| ApiError::from(AuthError::Internal("account lookup failed".to_owned())))?
        .ok_or_else(|| AuthError::NotFound.into())
}

/// Best-effort hash-chained audit append for a sensitive control-plane action.
/// A failure is logged but must not fail the action the caller already completed
/// (the action's own durability — secret write, state transition — already
/// committed); the tamper-evident record is additive.
async fn audit_action(
    state: &AppState,
    caller: &CallerContext,
    event_type: &str,
    subject_type: &str,
    subject_id: &str,
    details: serde_json::Value,
) {
    if let Err(error) = state
        .service
        .record_audit(caller, event_type, subject_type, subject_id, &details)
        .await
    {
        eprintln!("audit append failed for {event_type} {subject_id}: {error}");
    }
}

// ---- Route certification promotion (M4 Slice 8) ---------------------------

#[derive(Debug, Deserialize)]
pub struct CertifyStartRequest {
    pub target: CertificationState,
}

#[derive(Debug, Deserialize)]
pub struct CertifyFinishRequest {
    pub challenge_id: String,
    pub target: CertificationState,
    /// Opaque WebAuthn assertion authorizing the promotion.
    pub assertion: serde_json::Value,
}

/// `POST /api/v1/venue-credentials/{id}/certify/start`
pub async fn certify_route_start(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<CertifyStartRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_owner(&caller)?;
    require_enrollment(&state)?;
    let action = ActionDescriptor::CertifyRoute {
        venue_account_id: id,
        target: request.target.wire().to_owned(),
    };
    Ok(Json(
        passkeys(&state)?.start_action(&caller, &action).await?,
    ))
}

/// `POST /api/v1/venue-credentials/{id}/certify/finish`
pub async fn certify_route_finish(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<CertifyFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_owner(&caller)?;
    let enrollment = require_enrollment(&state)?;
    let action = ActionDescriptor::CertifyRoute {
        venue_account_id: id.clone(),
        target: request.target.wire().to_owned(),
    };
    passkeys(&state)?
        .finish_action(&caller, &request.challenge_id, &action, request.assertion)
        .await?;
    let tenant = caller.tenant_context()?;
    let account = enrollment
        .set_certification(tenant, &id, request.target)
        .await
        .map_err(map_enrollment)?;
    audit_action(
        &state,
        &caller,
        "ROUTE_CERTIFIED",
        "venue_account",
        &id,
        serde_json::json!({ "target": account.certification_state.wire() }),
    )
    .await;
    Ok(Json(account))
}

// ---- Credential preflight (M4) --------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PreflightRequest {
    pub product: String,
}

#[derive(Debug, Serialize)]
pub struct PreflightReconciliation {
    pub status: String,
    pub balances_seen: u32,
    pub positions_seen: u32,
    pub open_orders_seen: u32,
    pub discrepancies: u32,
}

#[derive(Debug, Serialize)]
pub struct PreflightReport {
    pub account_id: String,
    pub product: String,
    /// Resulting connection status: `ELIGIBLE` or `PREFLIGHT_FAILED`.
    pub status: String,
    pub checks: Vec<String>,
    pub reconciliation: Option<PreflightReconciliation>,
}

/// `POST /api/v1/venue-credentials/{id}/preflight`
///
/// Validates a connection's declared permissions and reconciles it against the
/// venue, then records `ELIGIBLE`/`PREFLIGHT_FAILED`. Fail-closed: no `ELIGIBLE`
/// without both config and live reconciliation passing. 503 in PAPER.
pub async fn preflight_credential(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<PreflightRequest>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }
    let registry = state
        .connector_registry
        .clone()
        .ok_or_else(|| ApiError::from(AuthError::CredentialsUnavailable))?;
    let enrollment = state
        .enrollment
        .clone()
        .ok_or_else(|| ApiError::from(AuthError::CredentialsUnavailable))?;
    let tenant = caller.tenant_context()?;
    let account = state
        .venues
        .get(tenant, &id)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::from(AuthError::NotFound))?;

    let outcome = PreflightService::new(registry, state.reconciliation.clone(), enrollment)
        .run(tenant, state.mode, &account, &request.product)
        .await
        .map_err(map_preflight)?;

    Ok(Json(PreflightReport {
        account_id: id,
        product: request.product,
        status: outcome.status,
        checks: outcome.checks,
        reconciliation: outcome
            .reconciliation
            .map(|summary| PreflightReconciliation {
                status: summary.status,
                balances_seen: summary.balances_seen as u32,
                positions_seen: summary.positions_seen as u32,
                open_orders_seen: summary.open_orders_seen as u32,
                discrepancies: summary.discrepancies as u32,
            }),
    }))
}

// ---- Pilot arming control plane (M4 Slice 7) ------------------------------

/// A preflight-eligible route a tenant may arm.
#[derive(Debug, Serialize)]
pub struct EligibleRoute {
    pub venue_account_id: String,
    pub alias: String,
    pub product: String,
}

/// Current tenant pilot state plus the routes eligible for arming.
#[derive(Debug, Serialize)]
pub struct PilotStatus {
    pub state: String,
    pub eligible: Vec<EligibleRoute>,
}

#[derive(Debug, Deserialize)]
pub struct PilotRouteRequest {
    pub venue_account_id: String,
    pub product: String,
}

#[derive(Debug, Deserialize)]
pub struct PilotArmFinishRequest {
    pub challenge_id: String,
    pub venue_account_id: String,
    pub product: String,
    /// Opaque WebAuthn assertion authorizing the arm.
    pub assertion: serde_json::Value,
    /// Typed confirmation; must equal the account alias.
    pub confirmation: String,
}

const STATUS_ELIGIBLE: &str = "ELIGIBLE";

fn map_pilot_error(error: PilotError) -> ApiError {
    match error {
        PilotError::ConfirmationRequired => {
            AuthError::BadRequest("typed confirmation does not match the account alias")
        }
        PilotError::TenantUnavailable => AuthError::Forbidden,
        PilotError::NotPaperOnly => AuthError::Conflict("tenant is already past paper mode"),
        PilotError::NoEligibleAccount => AuthError::Conflict("no preflight-eligible account"),
        PilotError::AccountNotEligible => AuthError::Conflict("route is not preflight-eligible"),
        PilotError::NotEligible => AuthError::Conflict("tenant is not pilot-eligible"),
        PilotError::NotArmed => AuthError::Conflict("tenant is not armed"),
    }
    .into()
}

fn map_live_risk(error: LiveRiskDenied) -> ApiError {
    match error {
        LiveRiskDenied::NotArmed => AuthError::Conflict("tenant is not armed for live risk"),
        LiveRiskDenied::RouteNotEligible => AuthError::Conflict("route is not preflight-eligible"),
        LiveRiskDenied::RouteNotCertified => {
            AuthError::Conflict("route is not certified for pilot")
        }
    }
    .into()
}

fn require_owner(caller: &CallerContext) -> Result<(), ApiError> {
    caller.require_active()?;
    if caller.is_owner() {
        Ok(())
    } else {
        Err(AuthError::Forbidden.into())
    }
}

/// Build the current pilot status: tenant state plus eligible routes.
async fn pilot_status(state: &AppState, caller: &CallerContext) -> Result<PilotStatus, ApiError> {
    let current = state.service.pilot_state(caller).await?;
    let accounts = state
        .venues
        .list(caller.tenant_context()?)
        .await
        .map_err(internal)?;
    let eligible = accounts
        .into_iter()
        .flat_map(|account| {
            let alias = account.alias;
            let account_id = account.id;
            account
                .connections
                .into_iter()
                .filter(|connection| connection.status == STATUS_ELIGIBLE)
                .map(move |connection| EligibleRoute {
                    venue_account_id: account_id.clone(),
                    alias: alias.clone(),
                    product: connection.product,
                })
        })
        .collect();
    Ok(PilotStatus {
        state: current.wire().to_owned(),
        eligible,
    })
}

/// Emit a control-plane pilot event on the durable outbox.
async fn emit_pilot_event(
    state: &AppState,
    caller: &CallerContext,
    event: &str,
    next: PilotState,
) -> Result<(), ApiError> {
    state
        .outbox
        .append(
            caller.tenant_context()?,
            event,
            &caller.tenant_id,
            serde_json::json!({ "state": next.wire() }),
            Utc::now(),
        )
        .await
        .map_err(internal)?;
    Ok(())
}

/// `GET /api/v1/pilot`
pub async fn get_pilot(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(pilot_status(&state, &caller).await?))
}

/// `POST /api/v1/pilot/eligibility`
pub async fn grant_pilot_eligibility(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    require_owner(&caller)?;
    let accounts = state
        .venues
        .list(caller.tenant_context()?)
        .await
        .map_err(internal)?;
    let has_eligible = accounts.iter().any(|account| {
        account
            .connections
            .iter()
            .any(|connection| connection.status == STATUS_ELIGIBLE)
    });
    let current = state.service.pilot_state(&caller).await?;
    let next = current
        .grant_eligibility(has_eligible)
        .map_err(map_pilot_error)?;
    state.service.set_pilot_state(&caller, next).await?;
    emit_pilot_event(&state, &caller, "PILOT_ELIGIBILITY_GRANTED", next).await?;
    Ok(Json(pilot_status(&state, &caller).await?))
}

/// `POST /api/v1/pilot/arm/start`
pub async fn pilot_arm_start(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<PilotRouteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_owner(&caller)?;
    let action = ActionDescriptor::ArmPilot {
        venue_account_id: request.venue_account_id,
        product: request.product,
    };
    Ok(Json(
        passkeys(&state)?.start_action(&caller, &action).await?,
    ))
}

/// `POST /api/v1/pilot/arm/finish`
pub async fn pilot_arm_finish(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<PilotArmFinishRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_owner(&caller)?;
    let action = ActionDescriptor::ArmPilot {
        venue_account_id: request.venue_account_id.clone(),
        product: request.product.clone(),
    };
    // Consume the one-use passkey proof before changing any state.
    passkeys(&state)?
        .finish_action(&caller, &request.challenge_id, &action, request.assertion)
        .await?;

    let account = require_enrollment_account(&state, &caller, &request.venue_account_id).await?;
    let eligible = account.connections.iter().any(|connection| {
        connection.product == request.product && connection.status == STATUS_ELIGIBLE
    });
    let confirmation_ok = request.confirmation.trim() == account.alias;
    let current = state.service.pilot_state(&caller).await?;
    let next = current
        .arm(eligible, confirmation_ok)
        .map_err(map_pilot_error)?;
    state.service.set_pilot_state(&caller, next).await?;
    emit_pilot_event(&state, &caller, "PILOT_ARMED", next).await?;
    audit_action(
        &state,
        &caller,
        "PILOT_ARMED",
        "venue_account",
        &request.venue_account_id,
        serde_json::json!({ "product": request.product }),
    )
    .await;
    Ok(Json(pilot_status(&state, &caller).await?))
}

/// `POST /api/v1/pilot/disarm` — always available; halts new risk.
pub async fn pilot_disarm(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    require_owner(&caller)?;
    let current = state.service.pilot_state(&caller).await?;
    let next = current.disarm().map_err(map_pilot_error)?;
    state.service.set_pilot_state(&caller, next).await?;
    // Reaching safety flattens open positions and blocks new risk.
    state
        .breakers
        .activate(
            caller.tenant_context()?,
            BreakerScope::Global,
            None,
            BreakerAction::Flatten,
            "OPERATOR",
            "pilot disarmed".to_owned(),
            Some(caller.user_id.clone()),
            Utc::now(),
        )
        .await
        .map_err(internal)?;
    emit_pilot_event(&state, &caller, "PILOT_DISARMED", next).await?;
    audit_action(
        &state,
        &caller,
        "PILOT_DISARMED",
        "tenant",
        &caller.tenant_id,
        serde_json::json!({}),
    )
    .await;
    Ok(Json(pilot_status(&state, &caller).await?))
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
    pub min_net_apr: Option<Decimal>,
    /// Minimum profitability probability, `[0, 1]`.
    pub min_probability: Option<Decimal>,
    /// Maximum entry basis magnitude, in basis points.
    pub max_basis_bps: Option<Decimal>,
    /// Maximum seconds until the next funding settlement.
    pub max_time_to_funding_secs: Option<i64>,
    /// Sort: `default`, `net_apr`, `next_funding`, `probability`, `capacity`, `expected_pnl`.
    pub sort: Option<String>,
}

fn seconds_to_funding(next_funding_at: &str, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(next_funding_at)
        .ok()
        .map(|parsed| {
            parsed
                .with_timezone(&Utc)
                .signed_duration_since(now)
                .num_seconds()
        })
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
        scan.opportunities
            .retain(|opportunity| opportunity.route_type == wanted);
    }
    if let Some(venue) = filter.venue.as_deref() {
        let wanted = venue.to_ascii_uppercase();
        scan.opportunities.retain(|opportunity| {
            opportunity.long_venue == wanted || opportunity.short_venue == wanted
        });
    }
    if let Some(underlying) = filter.underlying.as_deref() {
        let wanted = underlying.to_ascii_uppercase();
        scan.opportunities
            .retain(|opportunity| opportunity.underlying == wanted);
    }
    if let Some(direction) = filter.funding_direction.as_deref() {
        let wanted = direction.to_ascii_uppercase();
        scan.opportunities
            .retain(|opportunity| opportunity.funding_direction == wanted);
    }
    if let Some(minimum) = filter.min_net_apr {
        scan.opportunities
            .retain(|opportunity| opportunity.net_apr >= minimum);
    }
    if let Some(minimum) = filter.min_probability {
        scan.opportunities
            .retain(|opportunity| opportunity.profitability_probability >= minimum);
    }
    if let Some(maximum) = filter.max_basis_bps {
        scan.opportunities
            .retain(|opportunity| opportunity.entry_basis_bps <= maximum);
    }
    if let Some(maximum) = filter.max_time_to_funding_secs {
        let now = Utc::now();
        scan.opportunities.retain(|opportunity| {
            seconds_to_funding(&opportunity.next_funding_at, now).is_none_or(|secs| secs <= maximum)
        });
    }

    // `Reverse` keeps these descending, matching the previous `b.cmp(&a)` form.
    // Every sort is stable, so equal keys retain persisted rank order.
    match filter.sort.as_deref() {
        Some("net_apr") => scan.opportunities.sort_by_key(|row| Reverse(row.net_apr)),
        // Sorted by reference: `next_funding_at` is an RFC 3339 `String`, so a
        // `sort_by_key` would have to clone every key.
        Some("next_funding") => scan
            .opportunities
            .sort_by(|a, b| a.next_funding_at.cmp(&b.next_funding_at)),
        Some("probability") => scan
            .opportunities
            .sort_by_key(|row| Reverse(row.profitability_probability)),
        Some("capacity") => scan
            .opportunities
            .sort_by_key(|row| Reverse(row.available_capacity)),
        Some("expected_pnl") => scan
            .opportunities
            .sort_by_key(|row| Reverse(row.expected_net_pnl)),
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

fn not_found(title: &str, code: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
        format!(r#"{{"type":"about:blank","title":"{title}","status":404,"code":"{code}"}}"#),
    )
        .into_response()
}

fn conflict(title: &str, code: &str) -> Response {
    (
        StatusCode::CONFLICT,
        [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
        format!(r#"{{"type":"about:blank","title":"{title}","status":409,"code":"{code}"}}"#),
    )
        .into_response()
}

/// `POST /api/v1/positions` — open a paper position from a ranked route.
///
/// Requires an active OWNER or TRADER. Honors the `Idempotency-Key` header: a
/// repeat with the same key returns the existing position instead of opening a
/// duplicate.
pub async fn create_position(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    headers: HeaderMap,
    Json(request): Json<OpenPositionRequest>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }

    let supplied_key = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    if let Some(key) = supplied_key.as_deref()
        && let Some(existing) = state
            .positions
            .get_by_idempotency_key(caller.tenant_context()?, key)
            .await
            .map_err(internal)?
    {
        return Ok((StatusCode::OK, Json(existing)).into_response());
    }

    let key = supplied_key.unwrap_or_else(|| Uuid::new_v4().to_string());
    let notional = request.notional;
    let scenario = match request.fill_scenario.as_ref() {
        Some(value) => match value.to_domain() {
            Some(value) => value,
            None => {
                return Ok((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
                    r#"{"type":"about:blank","title":"Invalid fill scenario","status":422,"code":"INVALID_FILL_SCENARIO"}"#,
                )
                    .into_response());
            }
        },
        None => Default::default(),
    };
    let controls = state
        .breakers
        .controls(caller.tenant_context()?)
        .await
        .map_err(internal)?;
    let blocked = controls.breakers.iter().any(|breaker| {
        matches!(
            breaker.action,
            BreakerAction::ReadOnly | BreakerAction::Flatten | BreakerAction::HaltNewRisk
        ) || (breaker.action == BreakerAction::DisableVenue
            && breaker
                .scope_key
                .as_ref()
                .is_some_and(|venue| request.route_id.contains(venue)))
    });
    if blocked {
        return Ok((
            StatusCode::CONFLICT,
            [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
            r#"{"type":"about:blank","title":"New risk is halted","status":409,"code":"RISK_HALTED"}"#,
        )
            .into_response());
    }
    // Fill against the exact snapshot the latest scan ranked (DEC-0016), and
    // only while it is fresh: a stalled engine must not leave the API filling
    // paper orders against old books.
    let now = Utc::now();
    let snapshot = state.scanner.latest_snapshot().await.map_err(internal)?;
    let Some(snapshot) = snapshot.filter(|stored| {
        crate::freshness::is_fresh(stored.recorded_at, now, state.max_snapshot_age)
    }) else {
        return Ok(conflict("Market data is stale", "STALE_MARKET_DATA"));
    };
    let position = match state.execution.open_from(
        &snapshot.snapshot,
        &request.route_id,
        notional,
        key.clone(),
        scenario,
        now,
    ) {
        Ok(position) => position,
        Err(OpenError::RouteNotFound | OpenError::LegNotFound) => {
            return Ok(not_found("Route not open-eligible", "ROUTE_NOT_FOUND"));
        }
        Err(OpenError::LegDegraded) => {
            return Ok(conflict("A leg's venue feed is degraded", "VENUE_DEGRADED"));
        }
        Err(OpenError::InsufficientDepth) => {
            return Ok(conflict(
                "Order book depth cannot fill the requested size",
                "INSUFFICIENT_DEPTH",
            ));
        }
        Err(error @ OpenError::Fixture(_)) => {
            return Err(ApiError(AuthError::Internal(error.to_string())));
        }
    };

    let dto = PositionDto::from_domain(&position);
    match state
        .positions
        .create(caller.tenant_context()?, &dto, &key)
        .await
    {
        Ok(()) => Ok((StatusCode::CREATED, Json(dto)).into_response()),
        // Lost an idempotency race: return the position the other request stored.
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|db| db.is_unique_violation()) =>
        {
            match state
                .positions
                .get_by_idempotency_key(caller.tenant_context()?, &key)
                .await
                .map_err(internal)?
            {
                Some(existing) => Ok((StatusCode::OK, Json(existing)).into_response()),
                None => Err(internal(error)),
            }
        }
        Err(error) => Err(internal(error)),
    }
}

/// `GET /api/v1/positions` — list opened paper positions (newest first).
pub async fn list_positions(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    let positions = state
        .positions
        .list(caller.tenant_context()?)
        .await
        .map_err(internal)?;
    Ok(Json(PositionListDto { positions }))
}

/// `GET /api/v1/positions/{id}` — one paper position by id.
pub async fn get_position(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    match state
        .positions
        .get(caller.tenant_context()?, &id)
        .await
        .map_err(internal)?
    {
        Some(position) => Ok(Json(position).into_response()),
        None => Ok(not_found("Position not found", "POSITION_NOT_FOUND")),
    }
}

/// `POST /api/v1/positions/{id}/close` — close an open paper position.
///
/// Requires an active OWNER or TRADER. Idempotent: an already-closed position is
/// returned as-is; a non-open (`OPEN_FAILED`) position is rejected with `409`.
pub async fn close_position(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }
    let Some(dto) = state
        .positions
        .get(caller.tenant_context()?, &id)
        .await
        .map_err(internal)?
    else {
        return Ok(not_found("Position not found", "POSITION_NOT_FOUND"));
    };
    match dto.state.as_str() {
        "CLOSED" => return Ok((StatusCode::OK, Json(dto)).into_response()),
        "OPEN" => {}
        _ => {
            return Ok((
                StatusCode::CONFLICT,
                [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
                r#"{"type":"about:blank","title":"Position is not open","status":409,"code":"POSITION_NOT_OPEN"}"#,
            )
                .into_response());
        }
    }
    let closed = state.execution.close(dto.to_domain(), Utc::now());
    let closed_dto = PositionDto::from_domain(&closed);
    state
        .positions
        .update(caller.tenant_context()?, &closed_dto)
        .await
        .map_err(internal)?;
    Ok((StatusCode::OK, Json(closed_dto)).into_response())
}

#[derive(Deserialize)]
pub struct HaltRequest {
    #[serde(default = "halt_new_risk")]
    action: String,
    #[serde(default = "operator_reason")]
    reason: String,
}

fn halt_new_risk() -> String {
    "HALT_NEW_RISK".to_owned()
}

fn operator_reason() -> String {
    "Operator emergency control".to_owned()
}

#[derive(Deserialize)]
pub struct ControlReasonRequest {
    #[serde(default = "operator_reason")]
    reason: String,
}

/// `GET /api/v1/controls` — active durable paper breakers.
pub async fn get_controls(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        state
            .breakers
            .controls(caller.tenant_context()?)
            .await
            .map_err(internal)?,
    ))
}

/// `POST /api/v1/controls/halt` — halt new risk or enter read-only mode.
pub async fn halt(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<HaltRequest>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }
    let action = match request.action.as_str() {
        "HALT_NEW_RISK" => BreakerAction::HaltNewRisk,
        "READ_ONLY" => BreakerAction::ReadOnly,
        _ => {
            return Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
                r#"{"type":"about:blank","title":"Invalid halt action","status":422,"code":"INVALID_BREAKER_ACTION"}"#,
            )
                .into_response());
        }
    };
    activate_operator_breaker(
        &state,
        &caller,
        BreakerScope::Global,
        None,
        action,
        request.reason,
    )
    .await
}

/// `POST /api/v1/controls/venues/{venue}/disable`.
pub async fn disable_venue(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(venue): Path<String>,
    Json(request): Json<ControlReasonRequest>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }
    let venue = match Venue::from_str(&venue) {
        Ok(value) => value.as_str().to_owned(),
        Err(_) => return Ok(not_found("Venue not found", "VENUE_NOT_FOUND")),
    };
    activate_operator_breaker(
        &state,
        &caller,
        BreakerScope::Venue,
        Some(venue),
        BreakerAction::DisableVenue,
        request.reason,
    )
    .await
}

/// `POST /api/v1/controls/flatten` — OWNER-only emergency unwind request.
pub async fn flatten(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(request): Json<ControlReasonRequest>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.is_owner() {
        return Err(AuthError::Forbidden.into());
    }
    activate_operator_breaker(
        &state,
        &caller,
        BreakerScope::Global,
        None,
        BreakerAction::Flatten,
        request.reason,
    )
    .await
}

async fn activate_operator_breaker(
    state: &AppState,
    caller: &CallerContext,
    scope: BreakerScope,
    scope_key: Option<String>,
    action: BreakerAction,
    reason: String,
) -> Result<Response, ApiError> {
    let now = Utc::now();
    let breaker = state
        .breakers
        .activate(
            caller.tenant_context()?,
            scope,
            scope_key,
            action,
            "OPERATOR",
            reason,
            Some(caller.user_id.clone()),
            now,
        )
        .await
        .map_err(internal)?;
    state
        .outbox
        .append(
            caller.tenant_context()?,
            "CIRCUIT_BREAKER_ACTIVATED",
            &breaker.id,
            serde_json::to_value(&breaker)
                .map_err(|error| ApiError(AuthError::Internal(error.to_string())))?,
            now,
        )
        .await
        .map_err(internal)?;
    Ok((StatusCode::OK, Json(breaker)).into_response())
}

/// `POST /api/v1/controls/resume` — clear only operator-created controls.
pub async fn resume(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }
    let resolved = state
        .breakers
        .resolve_operator(caller.tenant_context()?, Utc::now())
        .await
        .map_err(internal)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "resolved": resolved })),
    )
        .into_response())
}

/// `GET /api/v1/strategies`.
pub async fn list_strategies(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        state
            .strategies
            .list(caller.tenant_context()?)
            .await
            .map_err(internal)?,
    ))
}

/// `GET /api/v1/strategies/{id}`.
pub async fn get_strategy(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    match state
        .strategies
        .get(caller.tenant_context()?, &id)
        .await
        .map_err(internal)?
    {
        Some(strategy) => Ok(Json(strategy).into_response()),
        None => Ok(not_found("Strategy not found", "STRATEGY_NOT_FOUND")),
    }
}

/// `POST /api/v1/strategies` creates version one.
pub async fn create_strategy(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Json(mut strategy): Json<StrategyDto>,
) -> Result<Response, ApiError> {
    require_strategy_write(&caller)?;
    strategy.id = Uuid::new_v4().to_string();
    strategy.current.version = 1;
    strategy.current.creator = caller.user_id.clone();
    strategy.current.created_at = Utc::now().to_rfc3339();
    strategy.current.mode = "PAPER".to_owned();
    validate_strategy(&strategy)?;
    state
        .strategies
        .create(caller.tenant_context()?, &strategy)
        .await
        .map_err(internal)?;
    Ok((StatusCode::CREATED, Json(strategy)).into_response())
}

/// `PUT /api/v1/strategies/{id}` creates an immutable new version.
pub async fn update_strategy(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(mut strategy): Json<StrategyDto>,
) -> Result<Response, ApiError> {
    require_strategy_write(&caller)?;
    let Some(existing) = state
        .strategies
        .get(caller.tenant_context()?, &id)
        .await
        .map_err(internal)?
    else {
        return Ok(not_found("Strategy not found", "STRATEGY_NOT_FOUND"));
    };
    strategy.id = id;
    strategy.enabled = existing.enabled;
    strategy.current.version = existing.current.version.saturating_add(1);
    strategy.current.creator = caller.user_id.clone();
    strategy.current.created_at = Utc::now().to_rfc3339();
    strategy.current.mode = "PAPER".to_owned();
    validate_strategy(&strategy)?;
    state
        .strategies
        .update(caller.tenant_context()?, &strategy)
        .await
        .map_err(internal)?;
    Ok(Json(strategy).into_response())
}

pub async fn delete_strategy(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_strategy_write(&caller)?;
    if state
        .strategies
        .delete(caller.tenant_context()?, &id)
        .await
        .map_err(internal)?
        == 0
    {
        return Ok(not_found("Strategy not found", "STRATEGY_NOT_FOUND"));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn set_strategy_enabled(
    state: &AppState,
    context: funding_arb_tenant_context::TenantContext,
    id: &str,
    enabled: bool,
) -> Result<Response, ApiError> {
    let Some(mut strategy) = state.strategies.get(context, id).await.map_err(internal)? else {
        return Ok(not_found("Strategy not found", "STRATEGY_NOT_FOUND"));
    };
    strategy.enabled = enabled;
    state
        .strategies
        .update(context, &strategy)
        .await
        .map_err(internal)?;
    Ok(Json(strategy).into_response())
}

pub async fn enable_strategy(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_strategy_write(&caller)?;
    set_strategy_enabled(&state, caller.tenant_context()?, &id, true).await
}

pub async fn disable_strategy(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_strategy_write(&caller)?;
    set_strategy_enabled(&state, caller.tenant_context()?, &id, false).await
}

pub async fn apply_strategy_to_open(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    require_strategy_write(&caller)?;
    let tenant = caller.tenant_context()?;
    if state
        .strategies
        .get(tenant, &id)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Ok(not_found("Strategy not found", "STRATEGY_NOT_FOUND"));
    }
    let mut applied = 0_u64;
    for mut position in state
        .positions
        .list(tenant)
        .await
        .map_err(internal)?
        .into_iter()
        .filter(|position| position.state == "OPEN")
    {
        position.strategy_id = Some(id.clone());
        state
            .positions
            .update(tenant, &position)
            .await
            .map_err(internal)?;
        applied += 1;
    }
    Ok(Json(serde_json::json!({"applied": applied})).into_response())
}

fn require_strategy_write(caller: &CallerContext) -> Result<(), ApiError> {
    caller.require_active()?;
    if caller.can_trade() {
        Ok(())
    } else {
        Err(AuthError::Forbidden.into())
    }
}

fn validate_strategy(strategy: &StrategyDto) -> Result<(), ApiError> {
    if strategy.name.trim().is_empty() || strategy.to_domain().is_none() {
        return Err(ApiError(AuthError::BadRequest(
            "invalid paper strategy configuration",
        )));
    }
    Ok(())
}

pub async fn get_portfolio(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        state
            .monitoring
            .portfolio(caller.tenant_context()?, Utc::now())
            .await
            .map_err(internal)?,
    ))
}

pub async fn get_health(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        state
            .monitoring
            .health(caller.tenant_context()?, Utc::now())
            .await
            .map_err(internal)?,
    ))
}

pub async fn list_alerts(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        state
            .monitoring
            .alerts(caller.tenant_context()?)
            .await
            .map_err(internal)?,
    ))
}

pub async fn ack_alert(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    match state
        .monitoring
        .acknowledge(caller.tenant_context()?, &id, &caller.user_id, Utc::now())
        .await
        .map_err(internal)?
    {
        Some(alert) => Ok(Json(alert).into_response()),
        None => Ok(not_found("Alert not found", "ALERT_NOT_FOUND")),
    }
}

/// `GET /api/v1/venues` — normalized connector capability catalog.
pub async fn list_venues(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    let accounts = state
        .venues
        .list(caller.tenant_context()?)
        .await
        .map_err(internal)?;
    let summary = [
        ("BYBIT", "Bybit", "TESTNET"),
        ("BINANCE", "Binance", "TESTNET"),
        ("OKX", "OKX", "DEMO"),
    ]
    .into_iter()
    .map(
        |(venue, display_name, sandbox_environment)| VenueSummaryDto {
            venue: venue.into(),
            display_name: display_name.into(),
            products: vec!["SPOT".into(), "LINEAR_PERPETUAL".into()],
            sandbox_environment: sandbox_environment.into(),
            configured_accounts: accounts
                .iter()
                .filter(|account| account.venue == venue)
                .count(),
        },
    )
    .collect::<Vec<_>>();
    Ok(Json(summary))
}

/// `GET /api/v1/venue-accounts` — masked account inventory.
pub async fn list_venue_accounts(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        state
            .venues
            .list(caller.tenant_context()?)
            .await
            .map_err(internal)?,
    ))
}

/// `GET /api/v1/connector-orders` — recent exact connector order intents.
pub async fn list_connector_orders(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        state
            .connector_monitoring
            .orders(caller.tenant_context()?)
            .await
            .map_err(internal)?,
    ))
}

/// `GET /api/v1/reconciliations` — recent connector reconciliation runs.
pub async fn list_reconciliations(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
) -> Result<impl IntoResponse, ApiError> {
    caller.require_active()?;
    Ok(Json(
        state
            .connector_monitoring
            .reconciliations(caller.tenant_context()?)
            .await
            .map_err(internal)?,
    ))
}

/// `GET /api/v1/venue-accounts/{id}/accounting` — latest exact venue snapshot.
pub async fn get_connector_accounting(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    let tenant = caller.tenant_context()?;
    if state
        .venues
        .get(tenant, &id)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Ok(not_found(
            "Venue account not found",
            "VENUE_ACCOUNT_NOT_FOUND",
        ));
    }
    Ok(Json(
        state
            .connector_monitoring
            .accounting(tenant, &id)
            .await
            .map_err(internal)?,
    )
    .into_response())
}

/// `GET /api/v1/venue-accounts/{id}` — masked account detail.
pub async fn get_venue_account(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    match state
        .venues
        .get(caller.tenant_context()?, &id)
        .await
        .map_err(internal)?
    {
        Some(account) => Ok(Json(account).into_response()),
        None => Ok(not_found(
            "Venue account not found",
            "VENUE_ACCOUNT_NOT_FOUND",
        )),
    }
}

/// `POST /api/v1/venue-accounts/{id}/test` — configuration-only safety test.
pub async fn test_venue_account(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }
    let tenant = caller.tenant_context()?;
    let Some(mut account) = state.venues.get(tenant, &id).await.map_err(internal)? else {
        return Ok(not_found(
            "Venue account not found",
            "VENUE_ACCOUNT_NOT_FOUND",
        ));
    };
    let now = Utc::now().to_rfc3339();
    let mut checks = Vec::new();
    let mut valid = account.enabled && !account.connections.is_empty();
    checks.push(
        if account.enabled {
            "ACCOUNT_ENABLED"
        } else {
            "ACCOUNT_DISABLED"
        }
        .into(),
    );
    for connection in &mut account.connections {
        let has_secret = connection.secret_ref.is_some();
        let no_withdrawal = connection.withdrawal_permission != Some(true);
        let environment_valid = match state.mode {
            funding_arb_domain::TradingMode::Sandbox => connection.environment != "PRODUCTION",
            funding_arb_domain::TradingMode::Shadow => {
                connection.environment == "PRODUCTION" && !connection.trade_permission
            }
            funding_arb_domain::TradingMode::Paper => true,
            funding_arb_domain::TradingMode::Live => false,
        };
        let connection_valid =
            has_secret && connection.read_permission && no_withdrawal && environment_valid;
        valid &= connection_valid;
        checks.push(format!(
            "{}:{}:{}",
            connection.product,
            connection.environment,
            if connection_valid { "VALID" } else { "INVALID" }
        ));
        connection.last_tested_at = Some(now.clone());
        connection.status = if connection_valid {
            "CONFIGURED".into()
        } else {
            "CONFIGURATION_FAILED".into()
        };
        state
            .venues
            .update_connection(tenant, &account.id, connection)
            .await
            .map_err(internal)?;
    }
    state
        .venues
        .update(tenant, &account)
        .await
        .map_err(internal)?;
    Ok(Json(ConnectionTestDto {
        account_id: account.id,
        status: if valid { "PASS" } else { "FAIL" }.into(),
        checked_at: now,
        checks,
    })
    .into_response())
}

/// `POST /api/v1/venue-accounts/{id}/disable` — Owner-only containment.
pub async fn disable_venue_account(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.is_owner() {
        return Err(AuthError::Forbidden.into());
    }
    let tenant = caller.tenant_context()?;
    let Some(mut account) = state.venues.get(tenant, &id).await.map_err(internal)? else {
        return Ok(not_found(
            "Venue account not found",
            "VENUE_ACCOUNT_NOT_FOUND",
        ));
    };
    account.enabled = false;
    account.certification_state = CertificationState::Disabled;
    for connection in &mut account.connections {
        connection.status = "DISABLED".into();
        state
            .venues
            .update_connection(tenant, &account.id, connection)
            .await
            .map_err(internal)?;
    }
    state
        .venues
        .update(tenant, &account)
        .await
        .map_err(internal)?;
    Ok(Json(account).into_response())
}

#[derive(Deserialize)]
pub struct ConnectorOrderRequest {
    product: String,
    symbol: String,
    side: String,
    base_quantity: String,
    limit_price: String,
    client_order_id: String,
    #[serde(default)]
    reduce_only: bool,
}

/// `POST /api/v1/venue-accounts/{id}/orders/preview` — exact native payload only.
pub async fn preview_connector_order(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<ConnectorOrderRequest>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }
    let (configured, intent) =
        connector_intent(&state, caller.tenant_context()?, &id, &request).await?;
    let prepared = state
        .connector_execution
        .preview(configured.connector.as_ref(), &intent)
        .await
        .map_err(|error| ApiError(AuthError::Internal(error.to_string())))?;
    Ok(Json(serde_json::json!({
        "venue": configured.account.venue,
        "environment": configured.environment.as_str(),
        "product": configured.product.as_str(),
        "native_symbol": prepared.native_symbol,
        "native_quantity": prepared.native_quantity.to_string(),
        "equivalent_base_quantity": prepared.equivalent_base_quantity.to_string(),
        "native_price": prepared.native_price.to_string(),
        "projected_delta_usd": prepared.projected_delta_usd.to_string(),
        "payload_hash": prepared.payload_hash,
        "payload": prepared.payload,
    }))
    .into_response())
}

/// `POST /api/v1/venue-accounts/{id}/orders` — sandbox-only durable submit-once.
pub async fn submit_connector_order(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
    Json(request): Json<ConnectorOrderRequest>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    if !caller.can_trade() {
        return Err(AuthError::Forbidden.into());
    }
    if !state.mode.allows_external_writes() {
        return Err(AuthError::Conflict("runtime mode forbids connector submission").into());
    }
    if state
        .breakers
        .controls(caller.tenant_context()?)
        .await
        .map_err(internal)?
        .effective_action
        .is_some()
    {
        return Err(AuthError::Conflict("circuit breaker blocks connector submission").into());
    }
    let tenant = caller.tenant_context()?;
    let (configured, intent) = connector_intent(&state, tenant, &id, &request).await?;
    // Live-risk gate: a production route may only take risk when the tenant is
    // armed, the route is preflight-eligible, and it is platform-certified.
    // Testnet/sandbox is exempt. Fail-closed and inert while LIVE is locked.
    if !configured.environment.is_non_production() {
        let pilot = state.service.pilot_state(&caller).await?;
        let eligible = configured.account.connections.iter().any(|connection| {
            connection.product == request.product && connection.status == "ELIGIBLE"
        });
        let certified = matches!(
            configured.account.certification_state,
            CertificationState::Pilot | CertificationState::CertifiedLive
        );
        permit_live_risk(pilot, eligible, certified).map_err(map_live_risk)?;
    }
    let result = state
        .connector_execution
        .submit_sandbox(
            tenant,
            state.mode,
            configured.environment,
            configured.connector.as_ref(),
            &intent,
        )
        .await
        .map_err(|error| ApiError(AuthError::Internal(error.to_string())))?;
    let status = match result.state {
        SubmissionState::Duplicate => StatusCode::OK,
        SubmissionState::Acknowledged => StatusCode::CREATED,
        SubmissionState::UnknownOutcome => StatusCode::ACCEPTED,
        SubmissionState::Failed => StatusCode::BAD_GATEWAY,
    };
    Ok((
        status,
        Json(serde_json::json!({
            "intent_id": result.intent_id,
            "state": match result.state {
                SubmissionState::Duplicate => "DUPLICATE",
                SubmissionState::Acknowledged => "ACKNOWLEDGED",
                SubmissionState::UnknownOutcome => "UNKNOWN_OUTCOME",
                SubmissionState::Failed => "FAILED",
            },
            "venue_order_id": result.acknowledgement.as_ref().and_then(|ack| ack.venue_order_id.clone()),
            "client_order_id": request.client_order_id,
        })),
    )
        .into_response())
}

async fn connector_intent(
    state: &AppState,
    tenant: funding_arb_tenant_context::TenantContext,
    account_id: &str,
    request: &ConnectorOrderRequest,
) -> Result<(ConfiguredConnector, CanonicalOrderIntent), ApiError> {
    let product = match request.product.as_str() {
        "SPOT" => ProductType::Spot,
        "LINEAR_PERPETUAL" => ProductType::LinearPerpetual,
        _ => return Err(AuthError::BadRequest("invalid connector product").into()),
    };
    let side = match request.side.as_str() {
        "BUY" => OrderSide::Buy,
        "SELL" => OrderSide::Sell,
        _ => return Err(AuthError::BadRequest("invalid connector order side").into()),
    };
    let base_quantity = Decimal::from_str(&request.base_quantity)
        .map_err(|_| AuthError::BadRequest("invalid base quantity"))?;
    let limit_price = Decimal::from_str(&request.limit_price)
        .map_err(|_| AuthError::BadRequest("invalid limit price"))?;
    if base_quantity <= Decimal::ZERO || limit_price <= Decimal::ZERO {
        return Err(AuthError::BadRequest("quantity and price must be positive").into());
    }
    if request.client_order_id.trim().is_empty() || request.client_order_id.len() > 32 {
        return Err(AuthError::BadRequest("invalid client order id").into());
    }
    let registry = state
        .connector_registry
        .as_ref()
        .ok_or(AuthError::Conflict("connector registry is unavailable"))?;
    let configured = registry
        .load(tenant, state.mode)
        .await
        .map_err(|error| ApiError(AuthError::Internal(error.to_string())))?
        .into_iter()
        .find(|configured| configured.account.id == account_id && configured.product == product)
        .ok_or(AuthError::NotFound)?;
    let symbol_allowed = matches!(
        (
            configured.account.venue.as_str(),
            product,
            request.symbol.as_str(),
        ),
        ("BINANCE" | "BYBIT", _, "BTCUSDT" | "ETHUSDT")
            | ("OKX", ProductType::Spot, "BTC-USDT" | "ETH-USDT")
            | (
                "OKX",
                ProductType::LinearPerpetual,
                "BTC-USDT-SWAP" | "ETH-USDT-SWAP"
            )
    );
    if !symbol_allowed {
        return Err(AuthError::BadRequest("instrument is outside the M3 allowlist").into());
    }
    let intent = CanonicalOrderIntent {
        account_id: AccountId(account_id.into()),
        instrument: InstrumentId {
            venue: configured.connector.venue(),
            symbol: request.symbol.clone(),
            product,
        },
        side,
        base_quantity,
        limit_price,
        time_in_force: "IOC".into(),
        reduce_only: request.reduce_only,
        client_order_id: request.client_order_id.clone(),
    };
    Ok((configured, intent))
}

fn internal(error: sqlx::Error) -> ApiError {
    ApiError(AuthError::Internal(error.to_string()))
}
