//! Application error type mapped by the transport layer to RFC 9457 problems.

/// A stable, transport-agnostic authentication error.
///
/// The `code` values are part of the client contract: the mobile app branches
/// on `REFRESH_TOKEN_REUSE`, `SESSION_REVOKED`, `USER_SUSPENDED`, and
/// `ACCOUNT_DELETED` to decide when to wipe the device keychain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthError {
    /// Malformed request the client can correct.
    BadRequest(&'static str),
    /// Provider identity token or PKCE proof failed verification.
    ProviderProofInvalid,
    /// The one-use challenge was already consumed or has expired.
    ChallengeConsumed,
    /// A refresh token from a prior generation was replayed.
    RefreshTokenReuse,
    /// The presented refresh token does not match an active session.
    InvalidRefreshToken,
    /// The session was revoked (including permission-version rotation).
    SessionRevoked,
    /// The account is suspended.
    UserSuspended,
    /// The account was deleted.
    AccountDeleted,
    /// Authentication is required but missing or invalid.
    Unauthorized,
    /// The caller is authenticated but not permitted.
    Forbidden,
    /// The requested resource does not exist.
    NotFound,
    /// A uniqueness or state conflict.
    Conflict(&'static str),
    /// The deployment bootstrap token was invalid or already consumed.
    BootstrapInvalid,
    /// An unexpected internal failure.
    Internal(String),
}

impl AuthError {
    /// HTTP status code for this error.
    #[must_use]
    pub const fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::ProviderProofInvalid
            | Self::RefreshTokenReuse
            | Self::InvalidRefreshToken
            | Self::SessionRevoked
            | Self::UserSuspended
            | Self::AccountDeleted
            | Self::BootstrapInvalid
            | Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::NotFound => 404,
            Self::ChallengeConsumed | Self::Conflict(_) => 409,
            Self::Internal(_) => 500,
        }
    }

    /// Stable machine-readable code included in the problem body.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::ProviderProofInvalid => "PROVIDER_PROOF_INVALID",
            Self::ChallengeConsumed => "CHALLENGE_CONSUMED",
            Self::RefreshTokenReuse => "REFRESH_TOKEN_REUSE",
            Self::InvalidRefreshToken => "INVALID_REFRESH_TOKEN",
            Self::SessionRevoked => "SESSION_REVOKED",
            Self::UserSuspended => "USER_SUSPENDED",
            Self::AccountDeleted => "ACCOUNT_DELETED",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Forbidden => "FORBIDDEN",
            Self::NotFound => "NOT_FOUND",
            Self::Conflict(_) => "CONFLICT",
            Self::BootstrapInvalid => "BOOTSTRAP_INVALID",
            Self::Internal(_) => "INTERNAL",
        }
    }

    /// Human-readable, non-sensitive title for this error.
    #[must_use]
    pub fn title(&self) -> String {
        match self {
            Self::BadRequest(detail) | Self::Conflict(detail) => (*detail).to_owned(),
            Self::ProviderProofInvalid => "The provider proof could not be verified.".to_owned(),
            Self::ChallengeConsumed => "The sign-in challenge is no longer valid.".to_owned(),
            Self::RefreshTokenReuse => "The session was invalidated for your safety.".to_owned(),
            Self::InvalidRefreshToken => "The refresh credential is not valid.".to_owned(),
            Self::SessionRevoked => "This session is no longer active.".to_owned(),
            Self::UserSuspended => "This account is suspended.".to_owned(),
            Self::AccountDeleted => "This account has been deleted.".to_owned(),
            Self::Unauthorized => "Authentication is required.".to_owned(),
            Self::Forbidden => "You do not have access to this resource.".to_owned(),
            Self::NotFound => "The requested resource was not found.".to_owned(),
            Self::BootstrapInvalid => "The deployment token was not accepted.".to_owned(),
            Self::Internal(_) => "The request could not be completed.".to_owned(),
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} ({})", self.code(), self.title())
    }
}

impl std::error::Error for AuthError {}

impl From<sqlx::Error> for AuthError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(format!("database error: {error}"))
    }
}

/// Convenience result alias for auth-service operations.
pub type AuthResult<T> = Result<T, AuthError>;
