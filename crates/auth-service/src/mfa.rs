//! TOTP verification and TOTP-secret encryption for MFA.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;

use crate::error::AuthError;

const STEP_SECONDS: u64 = 30;
const DIGITS: u32 = 6;
const BASE32_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Generate a fresh 160-bit TOTP secret encoded as base32.
#[must_use]
pub fn generate_secret() -> String {
    let mut bytes = [0_u8; 20];
    rand::thread_rng().fill_bytes(&mut bytes);
    base32_encode(&bytes)
}

/// Build the `otpauth://` provisioning URI for authenticator apps.
#[must_use]
pub fn otpauth_uri(issuer: &str, account: &str, secret_base32: &str) -> String {
    let issuer_enc = percent_encode(issuer);
    let account_enc = percent_encode(account);
    format!(
        "otpauth://totp/{issuer_enc}:{account_enc}?secret={secret_base32}&issuer={issuer_enc}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
    )
}

/// Verify a TOTP `code` against `secret_base32` at the current time.
///
/// Accepts the current step and one step either side. Returns the accepted
/// timestep so callers can reject reuse by persisting it.
///
/// # Errors
/// Returns [`AuthError::BadRequest`] if the secret cannot be decoded.
pub fn verify_totp(
    secret_base32: &str,
    code: &str,
    now_epoch_seconds: u64,
    last_accepted_timestep: Option<i64>,
) -> Result<Option<i64>, AuthError> {
    let secret = base32_decode(secret_base32)
        .ok_or(AuthError::BadRequest("stored TOTP secret is invalid"))?;
    let current = (now_epoch_seconds / STEP_SECONDS) as i64;
    for offset in [-1_i64, 0, 1] {
        let step = current + offset;
        if step < 0 {
            continue;
        }
        if let Some(last) = last_accepted_timestep {
            if step <= last {
                continue;
            }
        }
        if constant_time_eq(&totp_at(&secret, step as u64), code) {
            return Ok(Some(step));
        }
    }
    Ok(None)
}

fn totp_at(secret: &[u8], step: u64) -> String {
    type HmacSha1 = Hmac<Sha1>;
    let mut mac = HmacSha1::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let binary = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    let modulo = 10_u32.pow(DIGITS);
    format!("{:0width$}", binary % modulo, width = DIGITS as usize)
}

fn constant_time_eq(expected: &str, provided: &str) -> bool {
    use subtle::ConstantTimeEq;
    let provided = provided.trim();
    if expected.len() != provided.len() {
        return false;
    }
    expected.as_bytes().ct_eq(provided.as_bytes()).into()
}

/// Encrypt a TOTP secret with AES-256-GCM, prepending the random nonce.
///
/// # Errors
/// Returns [`AuthError::Internal`] if encryption fails.
pub fn encrypt_secret(key: &[u8; 32], plaintext: &str) -> Result<Vec<u8>, AuthError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0_u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| AuthError::Internal("TOTP secret encryption failed".to_owned()))?;
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a TOTP secret previously produced by [`encrypt_secret`].
///
/// # Errors
/// Returns [`AuthError::Internal`] if the ciphertext is malformed or invalid.
pub fn decrypt_secret(key: &[u8; 32], sealed: &[u8]) -> Result<String, AuthError> {
    if sealed.len() < 12 {
        return Err(AuthError::Internal("TOTP ciphertext too short".to_owned()));
    }
    let (nonce_bytes, ciphertext) = sealed.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| AuthError::Internal("TOTP secret decryption failed".to_owned()))?;
    String::from_utf8(plaintext)
        .map_err(|_| AuthError::Internal("TOTP secret not valid UTF-8".to_owned()))
}

fn base32_encode(input: &[u8]) -> String {
    let mut output = String::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for &byte in input {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let index = ((buffer >> bits) & 0x1f) as usize;
            output.push(BASE32_ALPHABET[index] as char);
        }
    }
    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0x1f) as usize;
        output.push(BASE32_ALPHABET[index] as char);
    }
    output
}

fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    let mut output = Vec::new();
    for character in input.trim_end_matches('=').chars() {
        let value = BASE32_ALPHABET
            .iter()
            .position(|&symbol| symbol == character.to_ascii_uppercase() as u8)?
            as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(output)
}

fn percent_encode(input: &str) -> String {
    let mut output = String::new();
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_round_trips() {
        let secret = generate_secret();
        let decoded = base32_decode(&secret).unwrap();
        assert_eq!(decoded.len(), 20);
    }

    #[test]
    fn generated_code_verifies_within_window() {
        let secret = generate_secret();
        let bytes = base32_decode(&secret).unwrap();
        let now = 1_700_000_000_u64;
        let step = now / STEP_SECONDS;
        let code = totp_at(&bytes, step);
        assert_eq!(
            verify_totp(&secret, &code, now, None).unwrap(),
            Some(step as i64)
        );
    }

    #[test]
    fn reused_timestep_is_rejected() {
        let secret = generate_secret();
        let bytes = base32_decode(&secret).unwrap();
        let now = 1_700_000_000_u64;
        let step = (now / STEP_SECONDS) as i64;
        let code = totp_at(&bytes, step as u64);
        assert_eq!(verify_totp(&secret, &code, now, Some(step)).unwrap(), None);
    }

    #[test]
    fn encryption_round_trips() {
        let key = [3_u8; 32];
        let sealed = encrypt_secret(&key, "JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(decrypt_secret(&key, &sealed).unwrap(), "JBSWY3DPEHPK3PXP");
    }
}
