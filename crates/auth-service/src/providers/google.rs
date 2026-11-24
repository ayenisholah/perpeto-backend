//! Google identity-token verification.

use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use serde::Deserialize;

use super::VerifiedIdentity;
use super::jwks::JwksCache;
use crate::error::AuthError;

/// Google's JWKS endpoint.
pub const JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const ISSUERS: [&str; 2] = ["accounts.google.com", "https://accounts.google.com"];

#[derive(Debug, Deserialize)]
struct GoogleClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<serde_json::Value>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

/// Verify a Google identity token against the accepted audiences (client ids).
///
/// The native iOS SDK never receives a nonce, so native replay defense is the
/// one-use challenge plus PKCE. Google Identity Services on the web does carry
/// the challenge nonce, and `exchange` requires it for web devices.
///
/// # Errors
/// Returns [`AuthError::ProviderProofInvalid`] on any verification failure.
pub async fn verify(
    cache: &JwksCache,
    audiences: &[String],
    identity_token: &str,
) -> Result<VerifiedIdentity, AuthError> {
    if audiences.is_empty() {
        return Err(AuthError::ProviderProofInvalid);
    }
    let header = decode_header(identity_token).map_err(|_| AuthError::ProviderProofInvalid)?;
    let kid = header.kid.ok_or(AuthError::ProviderProofInvalid)?;
    let key = cache.decoding_key(&kid).await?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&ISSUERS);
    validation.set_audience(audiences);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);

    let token = decode::<GoogleClaims>(identity_token, &key, &validation)
        .map_err(|_| AuthError::ProviderProofInvalid)?;
    let claims = token.claims;

    Ok(VerifiedIdentity {
        subject: claims.sub,
        email: claims.email,
        email_verified: matches!(
            claims.email_verified.as_ref(),
            Some(serde_json::Value::Bool(true))
        ) || matches!(
            claims.email_verified.as_ref(),
            Some(serde_json::Value::String(text)) if text == "true"
        ),
        display_name: claims.name,
        nonce: claims.nonce,
    })
}
