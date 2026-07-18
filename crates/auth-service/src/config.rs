//! Environment-driven configuration for the auth service.

use std::env;

use base64::{Engine as _, engine::general_purpose::STANDARD};

/// Static configuration resolved once at startup.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// Postgres connection string.
    pub database_url: String,
    /// TCP bind address for the HTTP server.
    pub bind_address: String,
    /// 32-byte Ed25519 signing seed for access and challenge tokens.
    pub token_signing_seed: [u8; 32],
    /// Issuer/audience claim for minted tokens.
    pub token_issuer: String,
    /// Expected Apple `aud` claim (the app bundle identifier).
    pub apple_bundle_id: String,
    /// Accepted Google `aud` claims (iOS and web client identifiers).
    pub google_client_ids: Vec<String>,
    /// SHA-256 (32 bytes) of the one-use deployment bootstrap token.
    pub bootstrap_token_hash: Option<[u8; 32]>,
    /// RFC 3339 expiry for the bootstrap token.
    pub bootstrap_token_expires_at: Option<String>,
    /// 32-byte AES-256-GCM key for encrypting TOTP secrets.
    pub mfa_encryption_key: Option<[u8; 32]>,
}

/// Reasons configuration could not be resolved.
#[derive(Clone, Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "configuration error: {}", self.0)
    }
}

impl std::error::Error for ConfigError {}

fn required(name: &str) -> Result<String, ConfigError> {
    env::var(name).map_err(|_| ConfigError(format!("{name} must be set")))
}

fn decode_key_32(name: &str, value: &str) -> Result<[u8; 32], ConfigError> {
    let bytes = STANDARD
        .decode(value.trim())
        .map_err(|error| ConfigError(format!("{name} is not valid base64: {error}")))?;
    bytes
        .try_into()
        .map_err(|_| ConfigError(format!("{name} must decode to exactly 32 bytes")))
}

fn decode_hex_32(name: &str, value: &str) -> Result<[u8; 32], ConfigError> {
    let trimmed = value.trim();
    if trimmed.len() != 64 {
        return Err(ConfigError(format!("{name} must be 64 hex characters")));
    }
    let mut out = [0_u8; 32];
    for (index, chunk) in trimmed.as_bytes().chunks_exact(2).enumerate() {
        let hi = hex_nibble(chunk[0]).ok_or_else(|| ConfigError(format!("{name} is not hex")))?;
        let lo = hex_nibble(chunk[1]).ok_or_else(|| ConfigError(format!("{name} is not hex")))?;
        out[index] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl AuthConfig {
    /// Resolve configuration from the process environment.
    ///
    /// # Errors
    /// Returns [`ConfigError`] when a required variable is missing or malformed.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = required("DATABASE_URL")?;
        let bind_address =
            env::var("SIGNEX_API_BIND").unwrap_or_else(|_| String::from("0.0.0.0:8080"));
        let token_signing_seed = decode_key_32(
            "SIGNEX_ACCESS_TOKEN_KEY",
            &required("SIGNEX_ACCESS_TOKEN_KEY")?,
        )?;
        let token_issuer = required("SIGNEX_TOKEN_ISSUER")?;
        let apple_bundle_id = required("SIGNEX_APPLE_BUNDLE_ID")?;

        let mut google_client_ids = Vec::new();
        if let Ok(value) = env::var("SIGNEX_GOOGLE_IOS_CLIENT_ID") {
            if !value.trim().is_empty() {
                google_client_ids.push(value.trim().to_owned());
            }
        }
        if let Ok(value) = env::var("SIGNEX_GOOGLE_WEB_CLIENT_ID") {
            if !value.trim().is_empty() {
                google_client_ids.push(value.trim().to_owned());
            }
        }

        let bootstrap_token_hash = match env::var("SIGNEX_BOOTSTRAP_TOKEN_HASH") {
            Ok(value) if !value.trim().is_empty() => {
                Some(decode_hex_32("SIGNEX_BOOTSTRAP_TOKEN_HASH", &value)?)
            }
            _ => None,
        };
        let bootstrap_token_expires_at = env::var("SIGNEX_BOOTSTRAP_TOKEN_EXPIRES_AT")
            .ok()
            .filter(|value| !value.trim().is_empty());

        let mfa_encryption_key = match env::var("SIGNEX_MFA_KEY") {
            Ok(value) if !value.trim().is_empty() => Some(decode_key_32("SIGNEX_MFA_KEY", &value)?),
            _ => None,
        };

        Ok(Self {
            database_url,
            bind_address,
            token_signing_seed,
            token_issuer,
            apple_bundle_id,
            google_client_ids,
            bootstrap_token_hash,
            bootstrap_token_expires_at,
            mfa_encryption_key,
        })
    }
}
