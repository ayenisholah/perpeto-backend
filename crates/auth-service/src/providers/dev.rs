//! Development-only identity verifier.
//!
//! # Why this exists
//!
//! Expo Go cannot perform Google or Apple sign-in: both are native modules it
//! does not ship. Without a substitute there is no way to reach the app's
//! authenticated surfaces from Expo Go at all, because every screen needs a real
//! access token.
//!
//! Every other check in [`crate::service::AuthService::exchange`] still runs
//! unchanged — challenge lifetime and purpose, the PKCE verifier, the device
//! platform, and (for Apple) the nonce binding. Only the provider signature
//! check is substituted, so the exchange still issues an ordinary session
//! through the ordinary code path.
//!
//! # This is an authentication bypass
//!
//! It accepts a caller-supplied identity as genuine. Enabling it anywhere real
//! would let anyone sign in as anyone. It is therefore selected only when
//! `PERPETO_MODE=PAPER` *and* `PERPETO_DEV_AUTH=true`, and `bins/api` refuses to
//! start if the flag is set in any other mode rather than quietly falling back
//! to the live verifier. `PERPETO_DEV_AUTH` must never appear in
//! `runtime.env.example`, Compose manifests, or CI — only in the gitignored
//! `deploy/dev.env`. See DEC-0014.

use crate::dto::Provider;
use crate::error::AuthError;
use crate::providers::{IdentityTokenVerifier, VerifiedIdentity};

/// The only token shape this verifier accepts: `perpeto-dev:<subject>[:<email>]`.
const PREFIX: &str = "perpeto-dev:";

/// Accepts `perpeto-dev:` tokens and rejects everything else.
pub struct DevIdentityVerifier;

#[async_trait::async_trait]
impl IdentityTokenVerifier for DevIdentityVerifier {
    async fn verify(
        &self,
        _provider: Provider,
        identity_token: &str,
    ) -> Result<VerifiedIdentity, AuthError> {
        // Reject anything that is not explicitly a development token. A real
        // Apple or Google token must not be treated as verified here just
        // because verification was relaxed.
        let claims = identity_token
            .strip_prefix(PREFIX)
            .ok_or(AuthError::ProviderProofInvalid)?;
        let mut parts = claims.splitn(2, ':');
        let subject = parts
            .next()
            .filter(|value| !value.is_empty())
            .ok_or(AuthError::ProviderProofInvalid)?;
        let email = parts.next().filter(|value| !value.is_empty());

        Ok(VerifiedIdentity {
            subject: subject.to_owned(),
            email: email.map(str::to_owned),
            email_verified: email.is_some(),
            display_name: Some("Development account".to_owned()),
            // Apple's nonce binding cannot be satisfied without the challenge,
            // so the development client signs in as GOOGLE, where `exchange`
            // does not consult the nonce.
            nonce: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepts_a_development_token_with_a_stable_subject() {
        let identity = DevIdentityVerifier
            .verify(
                Provider::Google,
                "perpeto-dev:local-owner:owner@example.test",
            )
            .await
            .expect("development token accepted");
        assert_eq!(identity.subject, "local-owner");
        assert_eq!(identity.email.as_deref(), Some("owner@example.test"));
        assert!(identity.email_verified);
    }

    #[tokio::test]
    async fn accepts_a_subject_without_an_email() {
        let identity = DevIdentityVerifier
            .verify(Provider::Google, "perpeto-dev:local-owner")
            .await
            .expect("development token accepted");
        assert_eq!(identity.subject, "local-owner");
        assert_eq!(identity.email, None);
        assert!(!identity.email_verified);
    }

    #[tokio::test]
    async fn rejects_tokens_without_the_development_prefix() {
        // A real provider token, or anything else, must not verify.
        for token in [
            "",
            "eyJhbGciOiJSUzI1NiIsImtpZCI6IjEyMyJ9.e30.sig",
            "perpeto-dev:",
        ] {
            assert_eq!(
                DevIdentityVerifier
                    .verify(Provider::Google, token)
                    .await
                    .err(),
                Some(AuthError::ProviderProofInvalid),
                "token must be rejected: {token:?}"
            );
        }
    }
}
