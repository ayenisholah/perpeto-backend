//! Encrypted credential storage. Plaintext credentials never enter API DTOs.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use funding_arb_tenant_context::{TenantContext, TenantTransaction};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{error::Error, fmt};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Raw venue credentials. This type must only be populated at the CLI boundary.
#[derive(Clone, Deserialize, Serialize, Zeroize, ZeroizeOnDrop)]
pub struct CredentialBundle {
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
}

impl fmt::Debug for CredentialBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialBundle([REDACTED])")
    }
}

impl CredentialBundle {
    /// Copy into the short-lived connector container; both copies zeroize on drop.
    #[must_use]
    pub fn connector_credentials(&self) -> funding_arb_connectors::ConnectorCredentials {
        funding_arb_connectors::ConnectorCredentials {
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            passphrase: self.passphrase.clone(),
        }
    }
}

/// Immutable binding used as authenticated encryption context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretContext {
    pub tenant: TenantContext,
    pub venue_account_id: Uuid,
    pub venue: String,
    pub product: String,
}

/// Non-secret metadata returned after a secret mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretMetadata {
    pub secret_ref: Uuid,
    pub key_version: i32,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
}

/// Deliberately redacted secret-store failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretStoreError {
    InvalidMasterKey,
    InvalidCredential,
    NotFound,
    ContextMismatch,
    EncryptionFailed,
    PersistenceFailed,
    VaultUnavailable,
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMasterKey => "invalid secret-store master key",
            Self::InvalidCredential => "invalid venue credential payload",
            Self::NotFound => "credential not found",
            Self::ContextMismatch => "credential context mismatch",
            Self::EncryptionFailed => "credential encryption operation failed",
            Self::PersistenceFailed => "credential persistence operation failed",
            Self::VaultUnavailable => "vault transit backend is unavailable",
        })
    }
}

impl Error for SecretStoreError {}

/// Credential persistence boundary, allowing a future KMS-backed implementation.
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn put(
        &self,
        context: &SecretContext,
        credential: &CredentialBundle,
    ) -> Result<SecretMetadata, SecretStoreError>;

    async fn load(
        &self,
        secret_ref: Uuid,
        context: &SecretContext,
    ) -> Result<CredentialBundle, SecretStoreError>;

    async fn rotate(
        &self,
        secret_ref: Uuid,
        context: &SecretContext,
        credential: &CredentialBundle,
    ) -> Result<SecretMetadata, SecretStoreError>;

    async fn revoke(&self, tenant: TenantContext, secret_ref: Uuid)
    -> Result<(), SecretStoreError>;
}

/// AES-256-GCM encrypted Postgres secret store.
pub struct EncryptedPostgresSecretStore {
    pool: PgPool,
    master_key: [u8; 32],
    key_version: i32,
}

impl EncryptedPostgresSecretStore {
    #[must_use]
    pub const fn new(pool: PgPool, master_key: [u8; 32], key_version: i32) -> Self {
        Self {
            pool,
            master_key,
            key_version,
        }
    }

    fn aad(context: &SecretContext, key_version: i32) -> Vec<u8> {
        format!(
            "perpeto-cex:{}:{}:{}:{}:{key_version}",
            context.tenant.tenant_id(),
            context.venue_account_id,
            context.venue,
            context.product
        )
        .into_bytes()
    }

    fn validate(credential: &CredentialBundle) -> Result<(), SecretStoreError> {
        if credential.api_key.trim().is_empty() || credential.api_secret.trim().is_empty() {
            return Err(SecretStoreError::InvalidCredential);
        }
        Ok(())
    }

    fn seal(
        &self,
        context: &SecretContext,
        credential: &CredentialBundle,
        key_version: i32,
    ) -> Result<([u8; 12], Vec<u8>, [u8; 32]), SecretStoreError> {
        Self::validate(credential)?;
        let mut plaintext =
            serde_json::to_vec(credential).map_err(|_| SecretStoreError::EncryptionFailed)?;
        let digest: [u8; 32] = Sha256::digest(&plaintext).into();
        let mut nonce = [0_u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let cipher = Aes256Gcm::new_from_slice(&self.master_key)
            .map_err(|_| SecretStoreError::InvalidMasterKey)?;
        let encrypted = cipher.encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: &Self::aad(context, key_version),
            },
        );
        plaintext.zeroize();
        let ciphertext = encrypted.map_err(|_| SecretStoreError::EncryptionFailed)?;
        Ok((nonce, ciphertext, digest))
    }

    fn open(
        &self,
        context: &SecretContext,
        key_version: i32,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<CredentialBundle, SecretStoreError> {
        let cipher = Aes256Gcm::new_from_slice(&self.master_key)
            .map_err(|_| SecretStoreError::InvalidMasterKey)?;
        let nonce: &[u8; 12] = nonce
            .try_into()
            .map_err(|_| SecretStoreError::EncryptionFailed)?;
        let mut plaintext = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &Self::aad(context, key_version),
                },
            )
            .map_err(|_| SecretStoreError::EncryptionFailed)?;
        let credential =
            serde_json::from_slice(&plaintext).map_err(|_| SecretStoreError::EncryptionFailed);
        plaintext.zeroize();
        credential
    }
}

#[async_trait]
impl SecretStore for EncryptedPostgresSecretStore {
    async fn put(
        &self,
        context: &SecretContext,
        credential: &CredentialBundle,
    ) -> Result<SecretMetadata, SecretStoreError> {
        let secret_ref = Uuid::new_v4();
        let (nonce, ciphertext, digest) = self.seal(context, credential, self.key_version)?;
        let mut transaction = TenantTransaction::begin(&self.pool, context.tenant)
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        let row = sqlx::query_as::<_, (DateTime<Utc>,)>(
            "INSERT INTO encrypted_credentials
             (tenant_id, secret_ref, venue_account_id, venue, product, key_version, nonce, ciphertext, plaintext_digest)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING created_at",
        )
        .bind(context.tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .bind(context.venue_account_id)
        .bind(&context.venue)
        .bind(&context.product)
        .bind(self.key_version)
        .bind(nonce.as_slice())
        .bind(ciphertext)
        .bind(digest.as_slice())
        .fetch_one(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?;
        transaction
            .commit()
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        Ok(SecretMetadata {
            secret_ref,
            key_version: self.key_version,
            created_at: row.0,
            rotated_at: None,
        })
    }

    async fn load(
        &self,
        secret_ref: Uuid,
        context: &SecretContext,
    ) -> Result<CredentialBundle, SecretStoreError> {
        let mut transaction = TenantTransaction::begin(&self.pool, context.tenant)
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        let row = sqlx::query_as::<_, (String, String, i32, Vec<u8>, Vec<u8>)>(
            "SELECT venue, product, key_version, nonce, ciphertext
             FROM encrypted_credentials
             WHERE tenant_id = $1 AND secret_ref = $2 AND revoked_at IS NULL",
        )
        .bind(context.tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .fetch_optional(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?
        .ok_or(SecretStoreError::NotFound)?;
        if row.0 != context.venue || row.1 != context.product {
            return Err(SecretStoreError::ContextMismatch);
        }
        let result = self.open(context, row.2, &row.3, &row.4);
        transaction
            .commit()
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        result
    }

    async fn rotate(
        &self,
        secret_ref: Uuid,
        context: &SecretContext,
        credential: &CredentialBundle,
    ) -> Result<SecretMetadata, SecretStoreError> {
        let (nonce, ciphertext, digest) = self.seal(context, credential, self.key_version)?;
        let mut transaction = TenantTransaction::begin(&self.pool, context.tenant)
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        let row = sqlx::query_as::<_, (DateTime<Utc>, DateTime<Utc>)>(
            "UPDATE encrypted_credentials
             SET key_version = $2, nonce = $3, ciphertext = $4, plaintext_digest = $5,
                 rotated_at = now()
             WHERE tenant_id = $9 AND secret_ref = $1 AND venue_account_id = $6 AND venue = $7 AND product = $8
               AND revoked_at IS NULL
             RETURNING created_at, rotated_at",
        )
        .bind(secret_ref)
        .bind(self.key_version)
        .bind(nonce.as_slice())
        .bind(ciphertext)
        .bind(digest.as_slice())
        .bind(context.venue_account_id)
        .bind(&context.venue)
        .bind(&context.product)
        .bind(context.tenant.tenant_id().as_uuid())
        .fetch_optional(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?
        .ok_or(SecretStoreError::NotFound)?;
        transaction
            .commit()
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        Ok(SecretMetadata {
            secret_ref,
            key_version: self.key_version,
            created_at: row.0,
            rotated_at: Some(row.1),
        })
    }

    async fn revoke(
        &self,
        tenant: TenantContext,
        secret_ref: Uuid,
    ) -> Result<(), SecretStoreError> {
        let mut transaction = TenantTransaction::begin(&self.pool, tenant)
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        let result = sqlx::query(
            "UPDATE encrypted_credentials SET revoked_at = now()
             WHERE tenant_id = $1 AND secret_ref = $2 AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .execute(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?;
        if result.rows_affected() == 0 {
            return Err(SecretStoreError::NotFound);
        }
        transaction
            .commit()
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)
    }
}

impl Drop for EncryptedPostgresSecretStore {
    fn drop(&mut self) {
        self.master_key.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use funding_arb_tenant_context::TenantId;
    use sqlx::postgres::PgPoolOptions;

    #[test]
    fn credential_debug_is_redacted() {
        let value = CredentialBundle {
            api_key: "visible-key".into(),
            api_secret: "visible-secret".into(),
            passphrase: None,
        };
        let rendered = format!("{value:?}");
        assert_eq!(rendered, "CredentialBundle([REDACTED])");
        assert!(!rendered.contains("visible"));
    }

    #[test]
    fn authenticated_context_prevents_cross_account_decryption() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres@localhost/perpeto")
            .expect("lazy pool");
        let store = EncryptedPostgresSecretStore::new(pool, [9_u8; 32], 1);
        let first = SecretContext {
            tenant: TenantContext::new(TenantId::new(Uuid::new_v4())),
            venue_account_id: Uuid::new_v4(),
            venue: "BYBIT".into(),
            product: "LINEAR_PERPETUAL".into(),
        };
        let second = SecretContext {
            venue_account_id: Uuid::new_v4(),
            ..first.clone()
        };
        let credential = CredentialBundle {
            api_key: "key".into(),
            api_secret: "secret".into(),
            passphrase: None,
        };
        let (nonce, ciphertext, _) = store.seal(&first, &credential, 1).expect("seal");
        assert!(store.open(&second, 1, &nonce, &ciphertext).is_err());
    }
}
