//! HashiCorp Vault Transit credential store (M4 Slice 5, code portion).
//!
//! This is the approved persistent cryptographic backend from
//! `docs/KEY_MANAGEMENT.md`: one non-exportable Transit key per tenant, only
//! ciphertext and masked metadata in PostgreSQL, and a record-binding
//! `associated_data` so ciphertext cannot be substituted across tenants,
//! accounts, venues or products. The API workload gets an **encrypt-only**
//! token; execution/reconciliation workloads get a **decrypt-only** token —
//! neither can manage keys.
//!
//! It implements the same [`SecretStore`](crate::secret::SecretStore) trait as
//! the transitional AES store, reuses the tenant-owned, forced-RLS
//! `encrypted_credentials` table (migration `0018` adds the `VAULT` row shape),
//! and persists exclusively through [`TenantTransaction`].
//!
//! Vault deployment, per-tenant key creation, Shamir init, policies and token
//! rotation are operator infrastructure and are intentionally out of scope
//! here. The Transit key must be an `aes256-gcm96` key so `associated_data` is
//! honored (Vault 1.13+).

use std::env;

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use funding_arb_tenant_context::{TenantContext, TenantTransaction};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::secret::{
    CredentialBundle, SecretContext, SecretMetadata, SecretStore, SecretStoreError,
};

/// Schema version mixed into the record binding. Bump only for a deliberate,
/// migrated change to the binding format; it is independent of the Vault key
/// version (which Vault assigns and rotates on its own).
const BINDING_SCHEMA: &str = "schema1";

/// Static Vault connection settings resolved once at startup.
#[derive(Clone, Debug)]
pub struct VaultConfig {
    /// Base address, e.g. `https://vault.internal:8200`.
    pub address: String,
    /// Transit secrets-engine mount path (default `transit`).
    pub mount: String,
    /// Encrypt-only token used by the API enrollment path.
    pub encrypt_token: String,
    /// Decrypt-only token used by execution/reconciliation workloads.
    pub decrypt_token: String,
    /// Rewrap-capable token used by the ciphertext-rewrap worker. Absent when no
    /// rewrap job is provisioned, in which case rewrap fails closed.
    pub rewrap_token: Option<String>,
    /// Prefix for per-tenant key names; keeps names opaque and namespaced.
    pub key_prefix: String,
    /// Optional Vault Enterprise namespace.
    pub namespace: Option<String>,
}

/// Reason Vault configuration could not be resolved.
#[derive(Clone, Debug)]
pub struct VaultConfigError(pub String);

impl std::fmt::Display for VaultConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "vault configuration error: {}", self.0)
    }
}

impl std::error::Error for VaultConfigError {}

impl VaultConfig {
    /// Resolve Vault configuration from the environment.
    ///
    /// Returns `Ok(None)` when `PERPETO_VAULT_ADDR` is unset (the deployment
    /// keeps the transitional AES store); `Ok(Some(_))` when Vault is selected;
    /// `Err` when Vault is selected but a required companion variable is missing.
    ///
    /// # Errors
    /// Returns [`VaultConfigError`] when a required variable is absent.
    pub fn from_env() -> Result<Option<Self>, VaultConfigError> {
        let Some(address) = non_empty("PERPETO_VAULT_ADDR") else {
            return Ok(None);
        };
        let required = |name: &str| {
            non_empty(name).ok_or_else(|| VaultConfigError(format!("{name} must be set")))
        };
        Ok(Some(Self {
            address: address.trim_end_matches('/').to_owned(),
            mount: non_empty("PERPETO_VAULT_TRANSIT_MOUNT").unwrap_or_else(|| "transit".to_owned()),
            encrypt_token: required("PERPETO_VAULT_ENCRYPT_TOKEN")?,
            decrypt_token: required("PERPETO_VAULT_DECRYPT_TOKEN")?,
            rewrap_token: non_empty("PERPETO_VAULT_REWRAP_TOKEN"),
            key_prefix: non_empty("PERPETO_VAULT_KEY_PREFIX").unwrap_or_else(|| "perpeto-".to_owned()),
            namespace: non_empty("PERPETO_VAULT_NAMESPACE"),
        }))
    }
}

fn non_empty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Vault Transit-backed secret store.
pub struct VaultTransitSecretStore {
    pool: PgPool,
    client: Client,
    config: VaultConfig,
}

impl VaultTransitSecretStore {
    #[must_use]
    pub fn new(pool: PgPool, config: VaultConfig) -> Self {
        Self {
            pool,
            client: Client::new(),
            config,
        }
    }

    /// Opaque per-tenant Transit key name — tenant UUID only, never email or
    /// exchange-account identifiers.
    fn key_name(&self, context: &SecretContext) -> String {
        format!("{}tenant-{}", self.config.key_prefix, context.tenant.tenant_id())
    }

    fn encrypt_url(&self, context: &SecretContext) -> String {
        format!(
            "{}/v1/{}/encrypt/{}",
            self.config.address,
            self.config.mount,
            self.key_name(context)
        )
    }

    fn decrypt_url(&self, context: &SecretContext) -> String {
        format!(
            "{}/v1/{}/decrypt/{}",
            self.config.address,
            self.config.mount,
            self.key_name(context)
        )
    }

    fn rewrap_url(&self, context: &SecretContext) -> String {
        format!(
            "{}/v1/{}/rewrap/{}",
            self.config.address,
            self.config.mount,
            self.key_name(context)
        )
    }

    /// Encrypt a validated credential and return the `vault:vN:...` ciphertext.
    async fn encrypt(
        &self,
        context: &SecretContext,
        credential: &CredentialBundle,
    ) -> Result<String, SecretStoreError> {
        validate(credential)?;
        let plaintext = Zeroizing::new(
            serde_json::to_vec(credential).map_err(|_| SecretStoreError::EncryptionFailed)?,
        );
        let request = TransitEncryptRequest {
            plaintext: STANDARD.encode(plaintext.as_slice()),
            associated_data: STANDARD.encode(associated_data(context)),
        };
        let response = self
            .send(&self.encrypt_url(context), &self.config.encrypt_token, &request)
            .await?;
        let parsed: TransitEncryptResponse =
            serde_json::from_str(&response).map_err(|_| SecretStoreError::EncryptionFailed)?;
        Ok(parsed.data.ciphertext)
    }

    /// Decrypt a stored ciphertext back into a credential bundle.
    async fn decrypt(
        &self,
        context: &SecretContext,
        ciphertext: &str,
    ) -> Result<CredentialBundle, SecretStoreError> {
        let request = TransitDecryptRequest {
            ciphertext: ciphertext.to_owned(),
            associated_data: STANDARD.encode(associated_data(context)),
        };
        let response = self
            .send(&self.decrypt_url(context), &self.config.decrypt_token, &request)
            .await?;
        let parsed: TransitDecryptResponse =
            serde_json::from_str(&response).map_err(|_| SecretStoreError::EncryptionFailed)?;
        let plaintext = Zeroizing::new(
            STANDARD
                .decode(parsed.data.plaintext.as_bytes())
                .map_err(|_| SecretStoreError::EncryptionFailed)?,
        );
        serde_json::from_slice(plaintext.as_slice()).map_err(|_| SecretStoreError::EncryptionFailed)
    }

    /// POST a JSON body to Vault with the given token; return the raw body.
    async fn send<B: Serialize>(
        &self,
        url: &str,
        token: &str,
        body: &B,
    ) -> Result<String, SecretStoreError> {
        let mut request = self.client.post(url).header("X-Vault-Token", token).json(body);
        if let Some(namespace) = &self.config.namespace {
            request = request.header("X-Vault-Namespace", namespace);
        }
        let response = request
            .send()
            .await
            .map_err(|_| SecretStoreError::VaultUnavailable)?;
        if !response.status().is_success() {
            return Err(SecretStoreError::VaultUnavailable);
        }
        response
            .text()
            .await
            .map_err(|_| SecretStoreError::VaultUnavailable)
    }
}

#[async_trait]
impl SecretStore for VaultTransitSecretStore {
    async fn put(
        &self,
        context: &SecretContext,
        credential: &CredentialBundle,
    ) -> Result<SecretMetadata, SecretStoreError> {
        let ciphertext = self.encrypt(context, credential).await?;
        let key_version = parse_key_version(&ciphertext);
        let secret_ref = Uuid::new_v4();
        let mut transaction = TenantTransaction::begin(&self.pool, context.tenant)
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        let row = sqlx::query_as::<_, (DateTime<Utc>,)>(
            "INSERT INTO encrypted_credentials
                 (tenant_id, secret_ref, venue_account_id, venue, product, key_version,
                  backend, vault_key_name, vault_ciphertext)
             VALUES ($1, $2, $3, $4, $5, $6, 'VAULT', $7, $8)
             RETURNING created_at",
        )
        .bind(context.tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .bind(context.venue_account_id)
        .bind(&context.venue)
        .bind(&context.product)
        .bind(key_version)
        .bind(self.key_name(context))
        .bind(&ciphertext)
        .fetch_one(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?;
        transaction
            .commit()
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        Ok(SecretMetadata {
            secret_ref,
            key_version,
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
        let row = sqlx::query_as::<_, (String, String, String)>(
            "SELECT venue, product, vault_ciphertext
             FROM encrypted_credentials
             WHERE tenant_id = $1 AND secret_ref = $2 AND backend = 'VAULT' AND revoked_at IS NULL",
        )
        .bind(context.tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .fetch_optional(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?
        .ok_or(SecretStoreError::NotFound)?;
        transaction
            .commit()
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        if row.0 != context.venue || row.1 != context.product {
            return Err(SecretStoreError::ContextMismatch);
        }
        self.decrypt(context, &row.2).await
    }

    async fn rotate(
        &self,
        secret_ref: Uuid,
        context: &SecretContext,
        credential: &CredentialBundle,
    ) -> Result<SecretMetadata, SecretStoreError> {
        let ciphertext = self.encrypt(context, credential).await?;
        let key_version = parse_key_version(&ciphertext);
        let mut transaction = TenantTransaction::begin(&self.pool, context.tenant)
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        let row = sqlx::query_as::<_, (DateTime<Utc>, DateTime<Utc>)>(
            "UPDATE encrypted_credentials
             SET key_version = $2, vault_ciphertext = $3, vault_key_name = $4, rotated_at = now()
             WHERE tenant_id = $8 AND secret_ref = $1 AND backend = 'VAULT'
               AND venue_account_id = $5 AND venue = $6 AND product = $7
               AND revoked_at IS NULL
             RETURNING created_at, rotated_at",
        )
        .bind(secret_ref)
        .bind(key_version)
        .bind(&ciphertext)
        .bind(self.key_name(context))
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
            key_version,
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
        // Rewrap requires a dedicated rewrap-capable token; fail closed without it.
        let Some(rewrap_token) = self.config.rewrap_token.as_deref() else {
            return Err(SecretStoreError::VaultUnavailable);
        };
        let mut transaction = TenantTransaction::begin(&self.pool, tenant)
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        let row = sqlx::query_as::<_, (Uuid, String, String, String, i32, DateTime<Utc>, Option<DateTime<Utc>>)>(
            "SELECT venue_account_id, venue, product, vault_ciphertext, key_version, created_at, rotated_at
             FROM encrypted_credentials
             WHERE tenant_id = $1 AND secret_ref = $2 AND backend = 'VAULT' AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .fetch_optional(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?
        .ok_or(SecretStoreError::NotFound)?;
        let (venue_account_id, venue, product, ciphertext, current_version, created_at, rotated_at) =
            row;
        let context = SecretContext {
            tenant,
            venue_account_id,
            venue,
            product,
        };

        // Transit rewraps ciphertext-only; plaintext never leaves Vault.
        let request = TransitRewrapRequest {
            ciphertext: ciphertext.clone(),
            associated_data: STANDARD.encode(associated_data(&context)),
        };
        let response = self
            .send(&self.rewrap_url(&context), rewrap_token, &request)
            .await?;
        let parsed: TransitEncryptResponse =
            serde_json::from_str(&response).map_err(|_| SecretStoreError::EncryptionFailed)?;
        let new_version = parse_key_version(&parsed.data.ciphertext);

        // Already at the latest key version: nothing to persist.
        if parsed.data.ciphertext == ciphertext || new_version == current_version {
            transaction
                .commit()
                .await
                .map_err(|_| SecretStoreError::PersistenceFailed)?;
            return Ok(SecretMetadata {
                secret_ref,
                key_version: current_version,
                created_at,
                rotated_at,
            });
        }

        sqlx::query(
            "UPDATE encrypted_credentials SET vault_ciphertext = $3, key_version = $4
             WHERE tenant_id = $1 AND secret_ref = $2 AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(secret_ref)
        .bind(&parsed.data.ciphertext)
        .bind(new_version)
        .execute(transaction.connection())
        .await
        .map_err(|_| SecretStoreError::PersistenceFailed)?;
        transaction
            .commit()
            .await
            .map_err(|_| SecretStoreError::PersistenceFailed)?;
        Ok(SecretMetadata {
            secret_ref,
            key_version: new_version,
            created_at,
            rotated_at,
        })
    }
}

#[derive(Serialize)]
struct TransitEncryptRequest {
    plaintext: String,
    associated_data: String,
}

#[derive(Serialize)]
struct TransitRewrapRequest {
    ciphertext: String,
    associated_data: String,
}

#[derive(Serialize)]
struct TransitDecryptRequest {
    ciphertext: String,
    associated_data: String,
}

#[derive(Deserialize)]
struct TransitEncryptResponse {
    data: TransitEncryptData,
}

#[derive(Deserialize)]
struct TransitEncryptData {
    ciphertext: String,
}

#[derive(Deserialize)]
struct TransitDecryptResponse {
    data: TransitDecryptData,
}

#[derive(Deserialize)]
struct TransitDecryptData {
    plaintext: String,
}

/// Non-secret record binding sent as Transit `associated_data`. Identical on
/// encrypt and decrypt; differs per tenant/account/venue/product so a
/// ciphertext cannot be moved to another record. The Vault key version is
/// deliberately excluded — Vault owns it and it is unknown before encryption.
fn associated_data(context: &SecretContext) -> Vec<u8> {
    format!(
        "perpeto-cex:{}:{}:{}:{}:{BINDING_SCHEMA}",
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

/// Parse the Transit key version from a `vault:vN:...` ciphertext, defaulting to
/// 1 for the unversioned/legacy form.
fn parse_key_version(ciphertext: &str) -> i32 {
    ciphertext
        .strip_prefix("vault:v")
        .and_then(|rest| rest.split_once(':'))
        .and_then(|(version, _)| version.parse::<i32>().ok())
        .filter(|version| *version > 0)
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use funding_arb_tenant_context::TenantId;

    fn context(account: Uuid, venue: &str, product: &str) -> SecretContext {
        SecretContext {
            tenant: TenantContext::new(TenantId::new(Uuid::nil())),
            venue_account_id: account,
            venue: venue.to_owned(),
            product: product.to_owned(),
        }
    }

    #[test]
    fn key_names_are_opaque_tenant_identifiers() {
        let config = VaultConfig {
            address: "https://vault.test".to_owned(),
            mount: "transit".to_owned(),
            encrypt_token: "e".to_owned(),
            decrypt_token: "d".to_owned(),
            rewrap_token: None,
            key_prefix: "perpeto-".to_owned(),
            namespace: None,
        };
        let tenant = Uuid::new_v4();
        let ctx = SecretContext {
            tenant: TenantContext::new(TenantId::new(tenant)),
            venue_account_id: Uuid::new_v4(),
            venue: "BYBIT".to_owned(),
            product: "LINEAR_PERPETUAL".to_owned(),
        };
        let store = VaultTransitSecretStore::new(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://postgres@localhost/perpeto")
                .expect("lazy pool"),
            config,
        );
        let name = store.key_name(&ctx);
        assert_eq!(name, format!("perpeto-tenant-{tenant}"));
        // No account/venue/product leakage in the key name.
        assert!(!name.contains("BYBIT"));
        assert!(!name.contains(&ctx.venue_account_id.to_string()));
    }

    #[test]
    fn associated_data_binds_each_record_distinctly() {
        let account = Uuid::new_v4();
        let base = associated_data(&context(account, "BYBIT", "LINEAR_PERPETUAL"));
        // A different account, venue or product yields a different binding.
        assert_ne!(base, associated_data(&context(Uuid::new_v4(), "BYBIT", "LINEAR_PERPETUAL")));
        assert_ne!(base, associated_data(&context(account, "BINANCE", "LINEAR_PERPETUAL")));
        assert_ne!(base, associated_data(&context(account, "BYBIT", "SPOT")));
        // Stable for the same record.
        assert_eq!(base, associated_data(&context(account, "BYBIT", "LINEAR_PERPETUAL")));
    }

    #[test]
    fn key_version_parses_from_ciphertext_prefix() {
        assert_eq!(parse_key_version("vault:v3:abcdef=="), 3);
        assert_eq!(parse_key_version("vault:v1:zzz"), 1);
        // Malformed or unversioned forms default to 1.
        assert_eq!(parse_key_version("not-a-vault-ciphertext"), 1);
        assert_eq!(parse_key_version("vault:vx:zzz"), 1);
    }

    #[test]
    fn empty_credentials_are_rejected_before_encryption() {
        assert_eq!(
            validate(&CredentialBundle {
                api_key: "  ".to_owned(),
                api_secret: "secret".to_owned(),
                passphrase: None,
            }),
            Err(SecretStoreError::InvalidCredential)
        );
    }

    // ---- Live Vault tests (skipped unless PERPETO_VAULT_ADDR is configured) ----
    //
    // These exercise the real Transit encrypt/decrypt round-trip and the
    // encrypt-only token restriction. The dev Vault must have an aes256-gcm96
    // Transit key named `perpeto-tenant-00000000-0000-0000-0000-000000000000`
    // (the nil-tenant key derived below) and the two policy-scoped tokens.

    fn lazy_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres@localhost/perpeto")
            .expect("lazy pool")
    }

    fn nil_context() -> SecretContext {
        SecretContext {
            tenant: TenantContext::new(TenantId::new(Uuid::nil())),
            venue_account_id: Uuid::nil(),
            venue: "BYBIT".to_owned(),
            product: "LINEAR_PERPETUAL".to_owned(),
        }
    }

    fn sample_credential() -> CredentialBundle {
        CredentialBundle {
            api_key: "round-trip-key".to_owned(),
            api_secret: "round-trip-secret".to_owned(),
            passphrase: Some("phrase".to_owned()),
        }
    }

    #[tokio::test]
    async fn vault_round_trip_encrypts_and_decrypts() {
        let Ok(Some(config)) = VaultConfig::from_env() else {
            eprintln!("skipping: PERPETO_VAULT_ADDR not set");
            return;
        };
        let store = VaultTransitSecretStore::new(lazy_pool(), config);
        let context = nil_context();
        let credential = sample_credential();
        let ciphertext = store.encrypt(&context, &credential).await.expect("encrypt");
        assert!(ciphertext.starts_with("vault:"), "expected a vault ciphertext");
        let recovered = store.decrypt(&context, &ciphertext).await.expect("decrypt");
        assert_eq!(recovered.api_key, credential.api_key);
        assert_eq!(recovered.api_secret, credential.api_secret);
        assert_eq!(recovered.passphrase, credential.passphrase);
    }

    #[tokio::test]
    async fn encrypt_only_token_cannot_decrypt() {
        let Ok(Some(config)) = VaultConfig::from_env() else {
            eprintln!("skipping: PERPETO_VAULT_ADDR not set");
            return;
        };
        let context = nil_context();
        let encrypter = VaultTransitSecretStore::new(lazy_pool(), config.clone());
        let ciphertext = encrypter
            .encrypt(&context, &sample_credential())
            .await
            .expect("encrypt");
        // A store whose decrypt path uses the encrypt-only token must be denied.
        let denied_config = VaultConfig {
            decrypt_token: config.encrypt_token.clone(),
            ..config
        };
        let denied = VaultTransitSecretStore::new(lazy_pool(), denied_config);
        assert!(
            matches!(
                denied.decrypt(&context, &ciphertext).await,
                Err(SecretStoreError::VaultUnavailable)
            ),
            "the encrypt-only token must not be able to decrypt"
        );
    }
}
