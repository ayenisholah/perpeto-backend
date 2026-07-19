//! Apple identity-token verification.

use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use serde::Deserialize;

use super::VerifiedIdentity;
use super::jwks::JwksCache;
use crate::error::AuthError;

/// Apple's JWKS endpoint.
pub const JWKS_URL: &str = "https://appleid.apple.com/auth/keys";
const ISSUER: &str = "https://appleid.apple.com";

#[derive(Debug, Deserialize)]
struct AppleClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<serde_json::Value>,
    #[serde(default)]
    nonce: Option<String>,
}

/// Verify an Apple identity token against the given audience (bundle id).
///
/// # Errors
/// Returns [`AuthError::ProviderProofInvalid`] on any verification failure.
pub async fn verify(
    cache: &JwksCache,
    audience: &str,
    identity_token: &str,
) -> Result<VerifiedIdentity, AuthError> {
    let header = decode_header(identity_token).map_err(|_| AuthError::ProviderProofInvalid)?;
    let kid = header.kid.ok_or(AuthError::ProviderProofInvalid)?;
    let key = cache.decoding_key(&kid).await?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[audience]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

    let token = decode::<AppleClaims>(identity_token, &key, &validation)
        .map_err(|_| AuthError::ProviderProofInvalid)?;
    let claims = token.claims;

    Ok(VerifiedIdentity {
        subject: claims.sub,
        email: claims.email,
        email_verified: coerce_bool(claims.email_verified.as_ref()),
        display_name: None,
        nonce: claims.nonce,
    })
}

fn coerce_bool(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(serde_json::Value::String(text)) => text == "true",
        _ => false,
    }
}
