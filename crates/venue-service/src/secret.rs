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

    /// Re-encrypt a stored secret to the store's current key version without
    /// changing the credential. Idempotent: a secret already at the current
    /// version is a no-op. Plaintext never leaves the store (Vault rewraps
    /// ciphertext-only; the AES store decrypts and re-encrypts internally).
    async fn rewrap(
        &self,
        tenant: TenantContext,
        secret_ref: Uuid,
    ) -> Result<SecretMetadata, SecretStoreError>;
}

/// One AES-256-GCM sealing: the random nonce, the ciphertext, and the digest of
/// the plaintext credential (used to detect a no-op rotation and to verify a
/// rewrap preserved the credential).
struct SealedSecret {
    nonce: [u8; 12],
    ciphertext: Vec<u8>,
    digest: [u8; 32],
}

/// AES-256-GCM encrypted Postgres secret store.
pub struct EncryptedPostgresSecretStore {
    pool: PgPool,
    master_key: [u8; 32],
    key_version: i32,
    /// Earlier key versions retained only to decrypt rows awaiting rewrap.
    previous_keys: Vec<(i32, [u8; 32])>,
}

impl EncryptedPostgresSecretStore {
    #[must_use]
    pub const fn new(pool: PgPool, master_key: [u8; 32], key_version: i32) -> Self {
        Self {
            pool,
            master_key,
            key_version,
            previous_keys: Vec::new(),
        }
    }

    /// Build a store that can also decrypt earlier key versions, enabling
    /// [`SecretStore::rewrap`] to migrate rows to the current `key_version`.
    #[must_use]
    pub fn with_previous_keys(
        pool: PgPool,
        master_key: [u8; 32],
        key_version: i32,
        previous_keys: Vec<(i32, [u8; 32])>,
    ) -> Self {
        Self {
            pool,
            master_key,
            key_version,
            previous_keys,
        }
    }

    /// Resolve the decryption key for a stored row's version: the current master
    /// key, or a retained earlier key. Unknown versions cannot be decrypted.
    fn decrypt_key(&self, key_version: i32) -> Result<&[u8; 32], SecretStoreError> {
        if key_version == self.key_version {
            return Ok(&self.master_key);
        }
        self.previous_keys
            .iter()
            .find(|(version, _)| *version == key_version)
            .map(|(_, key)| key)
            .ok_or(SecretStoreError::InvalidMasterKey)
    }

    /// Decrypt with an explicit key (used by rewrap for older key versions).
    fn open_with(
        key: &[u8; 32],
        context: &SecretContext,
        key_version: i32,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<CredentialBundle, SecretStoreError> {
        let cipher =
            Aes256Gcm::new_from_slice(key).map_err(|_| SecretStoreError::InvalidMasterKey)?;
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
    ) -> Result<SealedSecret, SecretStoreError> {
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
        Ok(SealedSecret {
            nonce,
            ciphertext,
            digest,
        })
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
        let SealedSecret {
            nonce,
            ciphertext,
            digest,
        } = self.seal(context, credential, self.key_version)?;
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
        let result = self
            .decrypt_key(row.2)
            .and_then(|key| Self::open_with(key, context, row.2, &row.3, &row.4));
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
        let SealedSecret {
            nonce,
            ciphertext,
            digest,
        } = self.seal(context, credential, self.key_version)?;
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

    async fn rewrap(
        &self,
        tenant: TenantContext,
        secret_ref: Uuid,
    ) -> Result<SecretMetadata, SecretStoreError> {
        let mut transaction = TenantTransaction::begin(&self.pool, tenant)
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        let row = sqlx::query_as::<_, (Uuid, String, String, i32, Vec<u8>, Vec<u8>, DateTime<Utc>, Option<DateTime<Utc>>)>(
            "SELECT venue_account_id, venue, product, key_version, nonce, ciphertext, created_at, rotated_at
             FROM encrypted_credentials
             WHERE tenant_id = $1 AND secret_ref = $2 AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .fetch_optional(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?
        .ok_or(SecretStoreError::NotFound)?;
        let (
            venue_account_id,
            venue,
            product,
            current_version,
            nonce,
            ciphertext,
            created_at,
            rotated_at,
        ) = row;

        // Already at the current key version: nothing to do.
        if current_version == self.key_version {
            transaction
                .commit()
                .await
                .map_err(|_| SecretStoreError::PersistenceFailed)?;
            return Ok(SecretMetadata {
                secret_ref,
                key_version: self.key_version,
                created_at,
                rotated_at,
            });
        }

        let context = SecretContext {
            tenant,
            venue_account_id,
            venue,
            product,
        };
        let key = self.decrypt_key(current_version)?;
        let credential = Self::open_with(key, &context, current_version, &nonce, &ciphertext)?;
        let SealedSecret {
            nonce: new_nonce,
            ciphertext: new_ciphertext,
            digest,
        } = self.seal(&context, &credential, self.key_version)?;
        // The credential is unchanged, so plaintext_digest stays identical.
        sqlx::query(
            "UPDATE encrypted_credentials
             SET key_version = $3, nonce = $4, ciphertext = $5, plaintext_digest = $6
             WHERE tenant_id = $1 AND secret_ref = $2 AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .bind(self.key_version)
        .bind(new_nonce.as_slice())
        .bind(new_ciphertext)
        .bind(digest.as_slice())
        .execute(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?;
        transaction
            .commit()
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        Ok(SecretMetadata {
            secret_ref,
            key_version: self.key_version,
            created_at,
            rotated_at,
        })
    }
}

impl Drop for EncryptedPostgresSecretStore {
    fn drop(&mut self) {
        self.master_key.zeroize();
        for (_, key) in &mut self.previous_keys {
            key.zeroize();
        }
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

    // Constructs a `PgPool`, which sqlx can only build inside a Tokio runtime.
    #[tokio::test]
    async fn rewrap_keyring_selects_current_or_retained_key() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres@localhost/perpeto")
            .expect("lazy pool");
        let store = EncryptedPostgresSecretStore::with_previous_keys(
            pool,
            [2_u8; 32],
            2,
            vec![(1, [1_u8; 32])],
        );
        assert_eq!(store.decrypt_key(2).expect("current"), &[2_u8; 32]);
        assert_eq!(store.decrypt_key(1).expect("previous"), &[1_u8; 32]);
        // An unknown version cannot be decrypted.
        assert_eq!(
            store.decrypt_key(3),
            Err(SecretStoreError::InvalidMasterKey)
        );
    }

    // Constructs a `PgPool`, which sqlx can only build inside a Tokio runtime.
    #[tokio::test]
    async fn authenticated_context_prevents_cross_account_decryption() {
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
        let SealedSecret {
            nonce, ciphertext, ..
        } = store.seal(&first, &credential, 1).expect("seal");
        assert!(
            EncryptedPostgresSecretStore::open_with(
                &store.master_key,
                &second,
                1,
                &nonce,
                &ciphertext
            )
            .is_err()
        );
    }
}
