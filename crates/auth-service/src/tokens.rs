//! Token minting and verification.
//!
//! Access and challenge tokens are compact EdDSA (Ed25519) JWTs signed by the
//! deployment key per DEC-0007. Refresh tokens are opaque high-entropy strings;
//! only their SHA-256 digest is ever persisted.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AuthError;

const ACCESS_TTL_SECONDS: i64 = 15 * 60;
const CHALLENGE_TTL_SECONDS: i64 = 10 * 60;

/// Claims embedded in an access token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccessClaims {
    /// Subject (user id).
    pub sub: String,
    /// Session id.
    pub sid: String,
    /// Device id.
    pub did: String,
    /// Permission version at mint time.
    pub pv: i64,
    /// Token issuer/audience.
    pub iss: String,
    pub aud: String,
    /// Issued-at and expiry (epoch seconds).
    pub iat: i64,
    pub exp: i64,
}

/// Claims embedded in a short-lived MFA challenge token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChallengeClaims {
    pub sub: String,
    /// Device id the eventual session should be bound to.
    pub did: String,
    /// `mfa_enroll` or `mfa`.
    pub purpose: String,
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Serialize, Deserialize)]
struct JwtHeader {
    alg: &'static str,
    typ: &'static str,
}

/// Signs and verifies deployment-issued tokens.
#[derive(Clone)]
pub struct TokenSigner {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    issuer: String,
}

impl TokenSigner {
    /// Construct a signer from a 32-byte seed and issuer/audience string.
    #[must_use]
    pub fn new(seed: [u8; 32], issuer: String) -> Self {
        let signing_key = SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        Self {
            signing_key,
            verifying_key,
            issuer,
        }
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn encode<T: Serialize>(&self, claims: &T) -> Result<String, AuthError> {
        let header = JwtHeader {
            alg: "EdDSA",
            typ: "JWT",
        };
        let header_json = serde_json::to_vec(&header)
            .map_err(|error| AuthError::Internal(format!("token header: {error}")))?;
        let claims_json = serde_json::to_vec(claims)
            .map_err(|error| AuthError::Internal(format!("token claims: {error}")))?;
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header_json),
            URL_SAFE_NO_PAD.encode(claims_json),
        );
        let signature: Signature = self.signing_key.sign(signing_input.as_bytes());
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        ))
    }

    fn decode<T: for<'de> Deserialize<'de>>(&self, token: &str) -> Result<T, AuthError> {
        let mut parts = token.split('.');
        let (Some(header_b64), Some(claims_b64), Some(signature_b64), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(AuthError::Unauthorized);
        };
        let signature_bytes = URL_SAFE_NO_PAD
            .decode(signature_b64)
            .map_err(|_| AuthError::Unauthorized)?;
        let signature_array: [u8; 64] = signature_bytes
            .as_slice()
            .try_into()
            .map_err(|_| AuthError::Unauthorized)?;
        let signature = Signature::from_bytes(&signature_array);
        let signing_input = format!("{header_b64}.{claims_b64}");
        self.verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| AuthError::Unauthorized)?;
        let claims_json = URL_SAFE_NO_PAD
            .decode(claims_b64)
            .map_err(|_| AuthError::Unauthorized)?;
        serde_json::from_slice(&claims_json).map_err(|_| AuthError::Unauthorized)
    }

    /// Mint an access token for a session.
    ///
    /// # Errors
    /// Returns [`AuthError::Internal`] if serialization fails.
    pub fn mint_access_token(
        &self,
        user_id: &str,
        session_id: &str,
        device_id: &str,
        permission_version: i64,
    ) -> Result<(String, i64), AuthError> {
        let iat = Self::now();
        let exp = iat + ACCESS_TTL_SECONDS;
        let claims = AccessClaims {
            sub: user_id.to_owned(),
            sid: session_id.to_owned(),
            did: device_id.to_owned(),
            pv: permission_version,
            iss: self.issuer.clone(),
            aud: self.issuer.clone(),
            iat,
            exp,
        };
        Ok((self.encode(&claims)?, exp))
    }

    /// Verify an access token and return its claims.
    ///
    /// # Errors
    /// Returns [`AuthError::Unauthorized`] if the token is invalid or expired.
    pub fn verify_access_token(&self, token: &str) -> Result<AccessClaims, AuthError> {
        let claims: AccessClaims = self.decode(token)?;
        if claims.iss != self.issuer || claims.aud != self.issuer {
            return Err(AuthError::Unauthorized);
        }
        if Self::now() >= claims.exp {
            return Err(AuthError::Unauthorized);
        }
        Ok(claims)
    }

    /// Mint a short-lived MFA challenge token.
    ///
    /// # Errors
    /// Returns [`AuthError::Internal`] if serialization fails.
    pub fn mint_challenge_token(
        &self,
        user_id: &str,
        device_id: &str,
        purpose: &str,
    ) -> Result<String, AuthError> {
        let iat = Self::now();
        let claims = ChallengeClaims {
            sub: user_id.to_owned(),
            did: device_id.to_owned(),
            purpose: purpose.to_owned(),
            iss: self.issuer.clone(),
            aud: self.issuer.clone(),
            iat,
            exp: iat + CHALLENGE_TTL_SECONDS,
        };
        self.encode(&claims)
    }

    /// Verify an MFA challenge token, requiring the given purpose.
    ///
    /// # Errors
    /// Returns [`AuthError::Unauthorized`] if the token is invalid, expired, or
    /// carries the wrong purpose.
    pub fn verify_challenge_token(
        &self,
        token: &str,
        expected_purpose: &str,
    ) -> Result<ChallengeClaims, AuthError> {
        let claims: ChallengeClaims = self.decode(token)?;
        if claims.iss != self.issuer
            || claims.aud != self.issuer
            || claims.purpose != expected_purpose
        {
            return Err(AuthError::Unauthorized);
        }
        if Self::now() >= claims.exp {
            return Err(AuthError::Unauthorized);
        }
        Ok(claims)
    }
}

/// Generate a fresh opaque refresh token (43+ URL-safe characters).
#[must_use]
pub fn generate_refresh_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate `count` recovery codes in `XXXX-XXXX` form.
#[must_use]
pub fn generate_recovery_codes(count: usize) -> Vec<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..count)
        .map(|_| {
            let mut code = String::with_capacity(9);
            for position in 0..8 {
                if position == 4 {
                    code.push('-');
                }
                let index = (rng.next_u32() as usize) % ALPHABET.len();
                code.push(ALPHABET[index] as char);
            }
            code
        })
        .collect()
}

/// SHA-256 digest of the input bytes.
#[must_use]
pub fn sha256(input: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher.finalize().into()
}

/// Decode a URL-safe base64 (no padding) string into bytes.
///
/// # Errors
/// Returns [`AuthError::BadRequest`] if the input is not valid base64url.
pub fn decode_base64url(input: &str) -> Result<Vec<u8>, AuthError> {
    URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|_| AuthError::BadRequest("value was not valid base64url"))
}

/// Encode bytes as URL-safe base64 without padding.
#[must_use]
pub fn encode_base64url(input: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> TokenSigner {
        TokenSigner::new([7_u8; 32], String::from("https://perpeto.test"))
    }

    #[test]
    fn access_tokens_round_trip() {
        let signer = signer();
        let (token, exp) = signer
            .mint_access_token("user-1", "session-1", "device-1", 3)
            .unwrap();
        let claims = signer.verify_access_token(&token).unwrap();
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.sid, "session-1");
        assert_eq!(claims.did, "device-1");
        assert_eq!(claims.pv, 3);
        assert_eq!(claims.exp, exp);
    }

    #[test]
    fn tampered_tokens_are_rejected() {
        let signer = signer();
        let (token, _) = signer
            .mint_access_token("user-1", "session-1", "device-1", 1)
            .unwrap();
        let mut tampered = token.clone();
        tampered.pop();
        tampered.push(if token.ends_with('A') { 'B' } else { 'A' });
        assert_eq!(
            signer.verify_access_token(&tampered),
            Err(AuthError::Unauthorized),
        );
    }

    #[test]
    fn foreign_key_cannot_verify() {
        let (token, _) = signer()
            .mint_access_token("user-1", "session-1", "device-1", 1)
            .unwrap();
        let other = TokenSigner::new([9_u8; 32], String::from("https://perpeto.test"));
        assert_eq!(
            other.verify_access_token(&token),
            Err(AuthError::Unauthorized),
        );
    }

    #[test]
    fn challenge_token_purpose_is_enforced() {
        let signer = signer();
        let token = signer
            .mint_challenge_token("user-1", "device-1", "mfa_enroll")
            .unwrap();
        assert!(signer.verify_challenge_token(&token, "mfa_enroll").is_ok());
        assert_eq!(
            signer.verify_challenge_token(&token, "mfa"),
            Err(AuthError::Unauthorized),
        );
    }

    #[test]
    fn refresh_tokens_are_high_entropy_and_unique() {
        let first = generate_refresh_token();
        let second = generate_refresh_token();
        assert!(first.len() >= 43);
        assert_ne!(first, second);
    }
}
