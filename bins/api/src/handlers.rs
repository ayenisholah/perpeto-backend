//! HTTP handlers translating requests into application-service calls.

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
use funding_arb_domain::market::Venue;
use funding_arb_domain::risk::RiskLimits;
use funding_arb_execution_service::{
    BreakerAction, BreakerScope, OpenError, OpenPositionRequest, PositionDto, PositionListDto,
};
use funding_arb_scanner_service::{OpportunityScanDto, RiskLimitsDto};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Deserialize;
use uuid::Uuid;

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

    match filter.sort.as_deref() {
        Some("net_apr") => scan
            .opportunities
            .sort_by(|a, b| b.net_apr.total_cmp(&a.net_apr)),
        Some("next_funding") => scan
            .opportunities
            .sort_by(|a, b| a.next_funding_at.cmp(&b.next_funding_at)),
        Some("probability") => scan.opportunities.sort_by(|a, b| {
            b.profitability_probability
                .total_cmp(&a.profitability_probability)
        }),
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

fn not_found(title: &str, code: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
        format!(r#"{{"type":"about:blank","title":"{title}","status":404,"code":"{code}"}}"#),
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
            .get_by_idempotency_key(key)
            .await
            .map_err(internal)?
    {
        return Ok((StatusCode::OK, Json(existing)).into_response());
    }

    let key = supplied_key.unwrap_or_else(|| Uuid::new_v4().to_string());
    let notional = request.notional.and_then(Decimal::from_f64);
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
    let controls = state.breakers.controls().await.map_err(internal)?;
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
    let position = match state.execution.open_with_scenario(
        &request.route_id,
        notional,
        key.clone(),
        scenario,
        Utc::now(),
    ) {
        Ok(position) => position,
        Err(OpenError::RouteNotFound | OpenError::LegNotFound) => {
            return Ok(not_found("Route not open-eligible", "ROUTE_NOT_FOUND"));
        }
        Err(error @ OpenError::Fixture(_)) => {
            return Err(ApiError(AuthError::Internal(error.to_string())));
        }
    };

    let dto = PositionDto::from_domain(&position);
    match state.positions.create(&dto, &key).await {
        Ok(()) => Ok((StatusCode::CREATED, Json(dto)).into_response()),
        // Lost an idempotency race: return the position the other request stored.
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|db| db.is_unique_violation()) =>
        {
            match state
                .positions
                .get_by_idempotency_key(&key)
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
    let positions = state.positions.list().await.map_err(internal)?;
    Ok(Json(PositionListDto { positions }))
}

/// `GET /api/v1/positions/{id}` — one paper position by id.
pub async fn get_position(
    State(state): State<AppState>,
    Extension(caller): Extension<CallerContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    caller.require_active()?;
    match state.positions.get(&id).await.map_err(internal)? {
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
    let Some(dto) = state.positions.get(&id).await.map_err(internal)? else {
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
        .update(&closed_dto)
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
    Ok(Json(state.breakers.controls().await.map_err(internal)?))
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
        .resolve_operator(Utc::now())
        .await
        .map_err(internal)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "resolved": resolved })),
    )
        .into_response())
}

fn internal(error: sqlx::Error) -> ApiError {
    ApiError(AuthError::Internal(error.to_string()))
}
