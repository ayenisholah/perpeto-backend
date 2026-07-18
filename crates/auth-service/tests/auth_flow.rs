//! End-to-end auth-service tests against a live Postgres database.
//!
//! These are skipped unless `DATABASE_URL` is set (as it is in CI, which runs a
//! Postgres service). Provider verification is faked through the
//! [`IdentityTokenVerifier`] trait so no live Apple/Google token is required.

use std::sync::Arc;

use async_trait::async_trait;
use funding_arb_auth_service::config::AuthConfig;
use funding_arb_auth_service::dto::{
    CreateChallengeRequest, DeviceRegistration, ExchangeRequest, NextState, Provider,
    RefreshRequest,
};
use funding_arb_auth_service::error::AuthError;
use funding_arb_auth_service::providers::{IdentityTokenVerifier, VerifiedIdentity};
use funding_arb_auth_service::service::CallerContext;
use funding_arb_auth_service::tokens::{encode_base64url, sha256};
use funding_arb_auth_service::{AuthService, Repo};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

struct FakeVerifier {
    identity: VerifiedIdentity,
}

#[async_trait]
impl IdentityTokenVerifier for FakeVerifier {
    async fn verify(
        &self,
        _provider: Provider,
        _identity_token: &str,
    ) -> Result<VerifiedIdentity, AuthError> {
        Ok(self.identity.clone())
    }
}

fn test_config(database_url: String, bootstrap_token: &str) -> AuthConfig {
    AuthConfig {
        database_url,
        bind_address: "127.0.0.1:0".to_owned(),
        token_signing_seed: [11_u8; 32],
        token_issuer: "https://signex.test".to_owned(),
        apple_bundle_id: "com.signex.mobile".to_owned(),
        google_client_ids: vec!["ios.apps.googleusercontent.com".to_owned()],
        bootstrap_token_hash: Some(sha256(bootstrap_token.as_bytes())),
        bootstrap_token_expires_at: Some("2999-01-01T00:00:00Z".to_owned()),
        mfa_encryption_key: Some([22_u8; 32]),
    }
}

async fn reset(pool: &PgPool) {
    sqlx::query(
        "TRUNCATE users, provider_identities, user_roles, social_challenges, devices, \
         sessions, refresh_token_history, mfa_methods, recovery_codes, \
         recovery_code_acknowledgements, bootstrap_state, step_up_proofs, \
         audit_actors, audit_events RESTART IDENTITY CASCADE",
    )
    .execute(pool)
    .await
    .expect("truncate");
}

async fn setup(subject: &str, bootstrap_token: &str) -> Option<(AuthService, PgPool)> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("connect");
    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("migrate");
    reset(&pool).await;

    let config = test_config(database_url, bootstrap_token);
    let verifier = Arc::new(FakeVerifier {
        identity: VerifiedIdentity {
            subject: subject.to_owned(),
            email: Some("operator@example.com".to_owned()),
            email_verified: true,
            display_name: Some("Operator".to_owned()),
            nonce: None,
        },
    });
    let service = AuthService::new(Repo::new(pool.clone()), &config, verifier);
    Some((service, pool))
}

fn device() -> DeviceRegistration {
    DeviceRegistration {
        installation_id: "install-123".to_owned(),
        platform: "IOS".to_owned(),
        app_version: "0.2.0".to_owned(),
        display_name: "Test iPhone".to_owned(),
    }
}

async fn sign_in(
    service: &AuthService,
    subject_verifier: &str,
) -> funding_arb_auth_service::dto::AuthTransition {
    let verifier = "verifier-000000000000000000000000000000000000000";
    let challenge_value = encode_base64url(&sha256(verifier.as_bytes()));
    let challenge = service
        .create_challenge(CreateChallengeRequest {
            provider: Provider::Google,
            redirect_uri: "signex://auth/callback".to_owned(),
            code_challenge: challenge_value,
            purpose: "SIGN_IN".to_owned(),
        })
        .await
        .expect("challenge");
    let _ = subject_verifier;
    service
        .exchange(ExchangeRequest {
            challenge_id: challenge.id,
            authorization_code: "auth-code".to_owned(),
            identity_token: "fake-token".to_owned(),
            code_verifier: Some(verifier.to_owned()),
            apple_profile: None,
            device: device(),
        })
        .await
        .expect("exchange")
}

#[tokio::test]
async fn new_user_reaches_pending_with_session() {
    let Some((service, _pool)) = setup("google-sub-pending", "dev-token").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let transition = sign_in(&service, "google-sub-pending").await;
    assert_eq!(transition.next, NextState::PendingApproval);
    assert!(
        transition.session.is_some(),
        "pending users get a session for bootstrap"
    );
    assert_eq!(
        transition.user.status,
        funding_arb_auth_service::dto::UserStatus::Pending
    );
}

#[tokio::test]
async fn refresh_rotates_and_detects_reuse() {
    let Some((service, _pool)) = setup("google-sub-refresh", "dev-token").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let transition = sign_in(&service, "google-sub-refresh").await;
    let session = transition.session.expect("session");

    let rotated = service
        .refresh(RefreshRequest {
            refresh_token: session.refresh_token.clone(),
            device_id: session.device_id.clone(),
        })
        .await
        .expect("first refresh");
    assert_ne!(rotated.refresh_token, session.refresh_token);

    // Replaying the original (now-consumed) token compromises the family.
    let reuse = service
        .refresh(RefreshRequest {
            refresh_token: session.refresh_token.clone(),
            device_id: session.device_id.clone(),
        })
        .await;
    assert_eq!(reuse.err(), Some(AuthError::RefreshTokenReuse));

    // The rotated token is now also invalid because the session is compromised.
    let after = service
        .refresh(RefreshRequest {
            refresh_token: rotated.refresh_token,
            device_id: session.device_id,
        })
        .await;
    assert!(after.is_err());
}

#[tokio::test]
async fn challenge_cannot_be_replayed() {
    let Some((service, _pool)) = setup("google-sub-replay", "dev-token").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    let verifier = "verifier-000000000000000000000000000000000000000";
    let challenge_value = encode_base64url(&sha256(verifier.as_bytes()));
    let challenge = service
        .create_challenge(CreateChallengeRequest {
            provider: Provider::Google,
            redirect_uri: "signex://auth/callback".to_owned(),
            code_challenge: challenge_value,
            purpose: "SIGN_IN".to_owned(),
        })
        .await
        .expect("challenge");
    let request = ExchangeRequest {
        challenge_id: challenge.id.clone(),
        authorization_code: "auth-code".to_owned(),
        identity_token: "fake-token".to_owned(),
        code_verifier: Some(verifier.to_owned()),
        apple_profile: None,
        device: device(),
    };
    service
        .exchange(request.clone())
        .await
        .expect("first exchange");
    let replay = service.exchange(request).await;
    assert_eq!(replay.err(), Some(AuthError::ChallengeConsumed));
}

#[tokio::test]
async fn bootstrap_promotes_first_owner_to_mfa_enrollment() {
    let Some((service, _pool)) = setup("google-sub-owner", "top-secret-deploy-token").await else {
        eprintln!("DATABASE_URL not set; skipping");
        return;
    };
    // Seed the bootstrap row (the API binary does this at startup).
    service
        .repo()
        .seed_bootstrap(
            &sha256("top-secret-deploy-token".as_bytes()),
            "2999-01-01T00:00:00Z",
        )
        .await
        .expect("seed");

    let transition = sign_in(&service, "google-sub-owner").await;
    let session = transition.session.expect("session");
    let caller = CallerContext {
        user_id: transition.user.id.clone(),
        session_id: session.session_id.clone(),
        device_id: session.device_id.clone(),
        status: "PENDING".to_owned(),
        roles: vec![],
    };

    let promoted = service
        .bootstrap(
            &caller,
            funding_arb_auth_service::dto::BootstrapRequest {
                deployment_token: "top-secret-deploy-token".to_owned(),
            },
        )
        .await
        .expect("bootstrap");
    assert_eq!(promoted.next, NextState::MfaEnrollmentRequired);
    assert!(promoted.challenge_token.is_some());
    assert!(
        promoted
            .user
            .roles
            .contains(&funding_arb_auth_service::dto::Role::Owner)
    );

    // The deployment token is single-use.
    let second = service
        .bootstrap(
            &caller,
            funding_arb_auth_service::dto::BootstrapRequest {
                deployment_token: "top-secret-deploy-token".to_owned(),
            },
        )
        .await;
    assert_eq!(second.err(), Some(AuthError::BootstrapInvalid));
}
