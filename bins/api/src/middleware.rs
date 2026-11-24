//! Bearer authentication middleware resolving the [`CallerContext`].

use axum::extract::{Request, State};
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::Response;
use funding_arb_auth_service::AuthError;
use funding_arb_auth_service::service::CallerContext;

use crate::problem::ApiError;
use crate::state::AppState;

/// Authenticate the request and attach a [`CallerContext`] extension.
///
/// The access token is verified for signature and expiry, then the caller's
/// live status and roles are read from the database so authorization always
/// reflects the current account state (not the token's mint-time snapshot).
///
/// # Errors
/// Returns 401 for missing or invalid credentials and account-status problems.
pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(AuthError::Unauthorized)?;

    let claims = state.service.verify_access_token(token)?;
    let user = state.service.user_projection(&claims.sub).await?;
    if user.tenant_id != claims.tid {
        return Err(AuthError::Unauthorized.into());
    }
    match user.status.as_str() {
        "DELETED" => return Err(AuthError::AccountDeleted.into()),
        "SUSPENDED" => return Err(AuthError::UserSuspended.into()),
        _ => {}
    }

    let context = CallerContext {
        user_id: claims.sub,
        tenant_id: claims.tid,
        session_id: claims.sid,
        device_id: claims.did,
        status: user.status,
        roles: user.roles,
        demo: user.demo,
    };
    // The demo principal is read-only (DEC-0017). Refusing every non-read
    // method here covers each protected route at once, including those whose
    // handlers only check that the account is active.
    if context.demo && !matches!(*request.method(), Method::GET | Method::HEAD) {
        return Err(AuthError::DemoReadOnly.into());
    }
    request.extensions_mut().insert(context);
    Ok(next.run(request).await)
}
