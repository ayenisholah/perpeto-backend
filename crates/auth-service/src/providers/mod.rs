//! Provider identity-token verification.
//!
//! Verification is expressed through the [`IdentityTokenVerifier`] trait so the
//! HTTP handlers can be exercised in tests with locally-signed tokens, while
//! production uses live Apple and Google JWKS.

pub mod apple;
pub mod dev;
pub mod google;
pub mod jwks;

use std::sync::Arc;

use crate::dto::Provider;
use crate::error::AuthError;
use jwks::JwksCache;

/// The verified, trustworthy claims extracted from a provider identity token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedIdentity {
    /// Stable provider subject identifier (`sub`).
    pub subject: String,
    /// Verified email address, if present.
    pub email: Option<String>,
    /// Whether the provider marked the email verified.
    pub email_verified: bool,
    /// Display name, if the provider supplied one.
    pub display_name: Option<String>,
    /// Raw nonce claim, if the provider token carries one.
    pub nonce: Option<String>,
}

/// Verifies a provider identity token and returns its trusted claims.
#[async_trait::async_trait]
pub trait IdentityTokenVerifier: Send + Sync {
    /// Verify an identity token for the given provider.
    ///
    /// # Errors
    /// Returns [`AuthError::ProviderProofInvalid`] when the token fails any
    /// signature, issuer, audience, or expiry check.
    async fn verify(
        &self,
        provider: Provider,
        identity_token: &str,
    ) -> Result<VerifiedIdentity, AuthError>;
}

/// Production verifier backed by live Apple and Google JWKS.
pub struct LiveIdentityVerifier {
    apple_cache: Arc<JwksCache>,
    google_cache: Arc<JwksCache>,
    apple_audiences: Vec<String>,
    google_client_ids: Vec<String>,
}

impl LiveIdentityVerifier {
    /// Construct the live verifier from configured audiences.
    #[must_use]
    pub fn new(apple_audiences: Vec<String>, google_client_ids: Vec<String>) -> Self {
        let http = reqwest::Client::new();
        Self {
            apple_cache: JwksCache::new(apple::JWKS_URL, http.clone()),
            google_cache: JwksCache::new(google::JWKS_URL, http),
            apple_audiences,
            google_client_ids,
        }
    }
}

#[async_trait::async_trait]
impl IdentityTokenVerifier for LiveIdentityVerifier {
    async fn verify(
        &self,
        provider: Provider,
        identity_token: &str,
    ) -> Result<VerifiedIdentity, AuthError> {
        match provider {
            Provider::Apple => {
                apple::verify(&self.apple_cache, &self.apple_audiences, identity_token).await
            }
            Provider::Google => {
                google::verify(&self.google_cache, &self.google_client_ids, identity_token).await
            }
        }
    }
}
