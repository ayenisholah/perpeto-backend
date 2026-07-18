//! Wire types mirroring `schemas/openapi.yaml` and the vendored api-client.
//!
//! Field names and shapes must stay byte-compatible with
//! `packages/api-client/src/index.ts`.

use serde::{Deserialize, Serialize};

/// Social identity provider as encoded on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Provider {
    Apple,
    Google,
}

/// Post-verification state the client should transition into.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NextState {
    PendingApproval,
    MfaRequired,
    MfaEnrollmentRequired,
    Authenticated,
}

/// Lifecycle status of a user account.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum UserStatus {
    Pending,
    Active,
    Suspended,
    Deleted,
}

/// A granted role.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Role {
    Viewer,
    Owner,
    Trader,
    Approver,
}

/// Public projection of the authenticated user.
#[derive(Clone, Debug, Serialize)]
pub struct CurrentUser {
    pub id: String,
    pub status: UserStatus,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub roles: Vec<Role>,
    pub linked_providers: Vec<Provider>,
    pub mfa_enrolled: bool,
}

/// The access + refresh credential pair returned after successful auth.
#[derive(Clone, Debug, Serialize)]
pub struct SessionCredentials {
    pub access_token: String,
    pub access_token_expires_at: String,
    pub refresh_token: String,
    pub session_id: String,
    pub device_id: String,
}

/// The transition envelope returned by exchange and bootstrap.
#[derive(Clone, Debug, Serialize)]
pub struct AuthTransition {
    pub next: NextState,
    pub user: CurrentUser,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionCredentials>,
}

/// Device fingerprint supplied at exchange time.
#[derive(Clone, Debug, Deserialize)]
pub struct DeviceRegistration {
    pub installation_id: String,
    pub platform: String,
    pub app_version: String,
    pub display_name: String,
}

/// Request body for challenge creation.
#[derive(Clone, Debug, Deserialize)]
pub struct CreateChallengeRequest {
    pub provider: Provider,
    pub redirect_uri: String,
    pub code_challenge: String,
    #[serde(default = "default_purpose")]
    pub purpose: String,
}

fn default_purpose() -> String {
    "SIGN_IN".to_owned()
}

/// Challenge response with the plaintext state and nonce.
#[derive(Clone, Debug, Serialize)]
pub struct SocialChallenge {
    pub id: String,
    pub provider: Provider,
    pub state: String,
    pub nonce: String,
    pub expires_at: String,
}

/// Optional Apple profile block (only present on first Apple sign-in).
#[derive(Clone, Debug, Deserialize)]
pub struct AppleProfile {
    pub display_name: Option<String>,
}

/// Request body for the social exchange endpoint.
#[derive(Clone, Debug, Deserialize)]
pub struct ExchangeRequest {
    pub challenge_id: String,
    pub authorization_code: String,
    pub identity_token: String,
    pub code_verifier: Option<String>,
    pub apple_profile: Option<AppleProfile>,
    pub device: DeviceRegistration,
}

/// Request body for refresh.
#[derive(Clone, Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
    pub device_id: String,
}

/// Request body for bootstrap.
#[derive(Clone, Debug, Deserialize)]
pub struct BootstrapRequest {
    pub deployment_token: String,
}

/// A device session projection.
#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub device_id: String,
    pub device_name: String,
    pub current: bool,
    pub created_at: String,
    pub last_seen_at: String,
}

/// A linked provider identity projection.
#[derive(Clone, Debug, Serialize)]
pub struct LinkedIdentity {
    pub provider: Provider,
    pub profile_email: Option<String>,
    pub linked_at: String,
    pub last_authenticated_at: String,
}

/// A pending access request projection for owners.
#[derive(Clone, Debug, Serialize)]
pub struct AccessRequest {
    pub user_id: String,
    pub provider: Provider,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub requested_at: String,
}

/// MFA enrollment material.
#[derive(Clone, Debug, Serialize)]
pub struct MfaEnrollment {
    pub method_id: String,
    pub secret: String,
    pub otpauth_uri: String,
}

/// Request body for MFA confirmation.
#[derive(Clone, Debug, Deserialize)]
pub struct MfaConfirmRequest {
    pub challenge_token: String,
    pub code: String,
}

/// Response body for MFA confirmation.
#[derive(Clone, Debug, Serialize)]
pub struct MfaConfirmResponse {
    pub recovery_codes: Vec<String>,
}

/// Request body for MFA verification.
#[derive(Clone, Debug, Deserialize)]
pub struct MfaVerifyRequest {
    pub challenge_token: String,
    pub code: String,
    #[serde(default)]
    pub recovery_code: bool,
    #[serde(default)]
    pub recovery_codes_acknowledged: bool,
}

/// Request body for recovery-code acknowledgement.
#[derive(Clone, Debug, Deserialize)]
pub struct AcknowledgeRecoveryRequest {
    pub challenge_token: String,
    pub code_set_digest: String,
}

impl From<funding_arb_domain::auth::SocialProvider> for Provider {
    fn from(value: funding_arb_domain::auth::SocialProvider) -> Self {
        match value {
            funding_arb_domain::auth::SocialProvider::Apple => Self::Apple,
            funding_arb_domain::auth::SocialProvider::Google => Self::Google,
        }
    }
}

impl From<Provider> for funding_arb_domain::auth::SocialProvider {
    fn from(value: Provider) -> Self {
        match value {
            Provider::Apple => Self::Apple,
            Provider::Google => Self::Google,
        }
    }
}

impl Provider {
    /// The Postgres enum label for this provider.
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Apple => "APPLE",
            Self::Google => "GOOGLE",
        }
    }

    /// Parse a provider from its Postgres enum label.
    #[must_use]
    pub fn from_db(value: &str) -> Option<Self> {
        match value {
            "APPLE" => Some(Self::Apple),
            "GOOGLE" => Some(Self::Google),
            _ => None,
        }
    }
}
