//! Passkey-authorized exchange-credential lifecycle (M4 Slice 6).
//!
//! This is the API replacement for the transitional `venue-admin` CLI: it
//! creates a venue account, writes the secret through the encrypt-only
//! [`SecretStore`] (Vault Transit outside PAPER), attaches only the returned
//! `secret_ref`, and returns a masked account — never credential material. The
//! passkey authorization gate lives at the transport layer (`bins/api`), which
//! verifies a one-use action proof before calling these methods; every read and
//! write here stays tenant-scoped through the repository and the store.
//!
//! Enrollment leaves each connection `PENDING_PREFLIGHT`. The worker that
//! performs live permission/reconcile validation, and the arming that promotes a
//! route to live, are later slices — this module only owns the state machine.

use funding_arb_tenant_context::TenantContext;
use sqlx::PgPool;
use uuid::Uuid;

use crate::dto::{
    CertificationState, ProductConnectionDto, ReconciliationState, VenueAccountDto,
};
use crate::registry::{RegistryError, secret_store_from_env};
use crate::secret::{CredentialBundle, SecretContext, SecretStore, SecretStoreError};
use crate::repo::VenueAccountsRepo;

/// Connection status values for the credential eligibility state machine.
const STATUS_PENDING_PREFLIGHT: &str = "PENDING_PREFLIGHT";
const STATUS_ELIGIBLE: &str = "ELIGIBLE";
const STATUS_PREFLIGHT_FAILED: &str = "PREFLIGHT_FAILED";
const STATUS_DISABLED: &str = "DISABLED";

/// A new-account enrollment request. Contains no secret material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnrollRequest {
    pub venue: String,
    pub alias: String,
    pub product: String,
    pub environment: String,
}

/// A deliberately redacted credential-lifecycle failure. Its `Display` never
/// includes credential material or persistence internals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnrollmentError {
    InvalidRequest,
    NotFound,
    Conflict,
    InvalidState,
    Secret(SecretStoreError),
    Persistence,
}

impl std::fmt::Display for EnrollmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "invalid credential enrollment request",
            Self::NotFound => "venue account or connection not found",
            Self::Conflict => "a venue account with this alias already exists",
            Self::InvalidState => "connection is not in a valid state for this action",
            Self::Secret(_) => "credential secret operation failed",
            Self::Persistence => "credential persistence failed",
        })
    }
}

impl std::error::Error for EnrollmentError {}

impl From<SecretStoreError> for EnrollmentError {
    fn from(error: SecretStoreError) -> Self {
        Self::Secret(error)
    }
}

/// Composes the venue-account repository and the credential [`SecretStore`].
pub struct CredentialEnrollmentService {
    accounts: VenueAccountsRepo,
    secrets: Box<dyn SecretStore>,
}

impl CredentialEnrollmentService {
    #[must_use]
    pub fn new(pool: PgPool, secrets: Box<dyn SecretStore>) -> Self {
        Self {
            accounts: VenueAccountsRepo::new(pool),
            secrets,
        }
    }

    /// Resolve the credential backend from the environment (Vault or AES),
    /// mirroring [`crate::ConnectorRegistry::from_env`].
    ///
    /// # Errors
    /// Returns [`RegistryError`] when no backend is configured.
    pub fn from_env(pool: PgPool) -> Result<Self, RegistryError> {
        let secrets = secret_store_from_env(pool.clone())?;
        Ok(Self::new(pool, secrets))
    }

    /// Enroll a new exchange credential, creating the venue account and
    /// attaching the secret reference. Returns a masked account.
    pub async fn enroll(
        &self,
        tenant: TenantContext,
        request: &EnrollRequest,
        credential: &CredentialBundle,
    ) -> Result<VenueAccountDto, EnrollmentError> {
        validate_request(request)?;
        let account_id = Uuid::new_v4();
        let connection = ProductConnectionDto {
            id: Uuid::new_v4().to_string(),
            product: request.product.clone(),
            environment: request.environment.clone(),
            secret_ref: None,
            read_permission: true,
            trade_permission: request.environment != "PRODUCTION",
            withdrawal_permission: Some(false),
            status: STATUS_PENDING_PREFLIGHT.to_owned(),
            last_tested_at: None,
        };
        let mut account = VenueAccountDto {
            id: account_id.to_string(),
            venue: request.venue.clone(),
            alias: request.alias.clone(),
            masked_identity: mask_key(&credential.api_key),
            enabled: true,
            certification_state: CertificationState::PaperOnly,
            reconciliation_state: ReconciliationState::Pending,
            connections: vec![connection],
            last_reconciled_at: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            tombstoned_at: None,
        };
        self.accounts
            .create(tenant, &account)
            .await
            .map_err(map_persist)?;

        let context = SecretContext {
            tenant,
            venue_account_id: account_id,
            venue: request.venue.clone(),
            product: request.product.clone(),
        };
        let metadata = self.secrets.put(&context, credential).await?;
        account.connections[0].secret_ref = Some(metadata.secret_ref.to_string());
        self.accounts
            .update_connection(tenant, &account.id, &account.connections[0])
            .await
            .map_err(|_| EnrollmentError::Persistence)?;
        Ok(account)
    }

    /// Rotate an existing connection's credential to a new version and require
    /// preflight revalidation.
    pub async fn rotate(
        &self,
        tenant: TenantContext,
        account_id: &str,
        product: &str,
        credential: &CredentialBundle,
    ) -> Result<VenueAccountDto, EnrollmentError> {
        let (mut account, index) = self.locate(tenant, account_id, product).await?;
        let secret_ref = self.connection_secret_ref(&account.connections[index])?;
        let context = SecretContext {
            tenant,
            venue_account_id: parse_uuid(&account.id)?,
            venue: account.venue.clone(),
            product: product.to_owned(),
        };
        self.secrets.rotate(secret_ref, &context, credential).await?;
        account.masked_identity = mask_key(&credential.api_key);
        account.connections[index].status = STATUS_PENDING_PREFLIGHT.to_owned();
        account.connections[index].last_tested_at = None;
        self.accounts
            .update_connection(tenant, &account.id, &account.connections[index])
            .await
            .map_err(|_| EnrollmentError::Persistence)?;
        self.accounts
            .update(tenant, &account)
            .await
            .map_err(|_| EnrollmentError::Persistence)?;
        Ok(account)
    }

    /// Revoke a connection's credential: destroy the stored secret reference,
    /// clear it, and disable the connection. Positions are left to reconcile.
    pub async fn revoke(
        &self,
        tenant: TenantContext,
        account_id: &str,
        product: &str,
    ) -> Result<VenueAccountDto, EnrollmentError> {
        let (mut account, index) = self.locate(tenant, account_id, product).await?;
        if let Some(secret_ref) = account.connections[index].secret_ref.clone() {
            let secret_ref = Uuid::parse_str(&secret_ref).map_err(|_| EnrollmentError::NotFound)?;
            self.secrets.revoke(tenant, secret_ref).await?;
        }
        account.connections[index].secret_ref = None;
        account.connections[index].status = STATUS_DISABLED.to_owned();
        self.accounts
            .update_connection(tenant, &account.id, &account.connections[index])
            .await
            .map_err(|_| EnrollmentError::Persistence)?;
        Ok(account)
    }

    /// Record the outcome of preflight validation, moving the connection from
    /// `PENDING_PREFLIGHT` to `ELIGIBLE` or `PREFLIGHT_FAILED`. The live
    /// validation itself is a deferred worker; this only owns the transition.
    pub async fn record_preflight_result(
        &self,
        tenant: TenantContext,
        account_id: &str,
        product: &str,
        eligible: bool,
    ) -> Result<VenueAccountDto, EnrollmentError> {
        let (mut account, index) = self.locate(tenant, account_id, product).await?;
        if account.connections[index].status != STATUS_PENDING_PREFLIGHT {
            return Err(EnrollmentError::InvalidState);
        }
        account.connections[index].status = if eligible {
            STATUS_ELIGIBLE
        } else {
            STATUS_PREFLIGHT_FAILED
        }
        .to_owned();
        account.connections[index].last_tested_at = Some(chrono::Utc::now().to_rfc3339());
        self.accounts
            .update_connection(tenant, &account.id, &account.connections[index])
            .await
            .map_err(|_| EnrollmentError::Persistence)?;
        Ok(account)
    }

    async fn locate(
        &self,
        tenant: TenantContext,
        account_id: &str,
        product: &str,
    ) -> Result<(VenueAccountDto, usize), EnrollmentError> {
        let account = self
            .accounts
            .get(tenant, account_id)
            .await
            .map_err(|_| EnrollmentError::Persistence)?
            .ok_or(EnrollmentError::NotFound)?;
        let index = account
            .connections
            .iter()
            .position(|connection| connection.product == product)
            .ok_or(EnrollmentError::NotFound)?;
        Ok((account, index))
    }

    fn connection_secret_ref(
        &self,
        connection: &ProductConnectionDto,
    ) -> Result<Uuid, EnrollmentError> {
        let secret_ref = connection
            .secret_ref
            .as_deref()
            .ok_or(EnrollmentError::NotFound)?;
        Uuid::parse_str(secret_ref).map_err(|_| EnrollmentError::NotFound)
    }
}

fn validate_request(request: &EnrollRequest) -> Result<(), EnrollmentError> {
    if request.venue.trim().is_empty()
        || request.alias.trim().is_empty()
        || request.product.trim().is_empty()
        || request.environment.trim().is_empty()
    {
        return Err(EnrollmentError::InvalidRequest);
    }
    Ok(())
}

fn parse_uuid(value: &str) -> Result<Uuid, EnrollmentError> {
    Uuid::parse_str(value).map_err(|_| EnrollmentError::NotFound)
}

/// A `venue_accounts_tenant_venue_alias_key` unique violation is a caller
/// conflict; anything else is an internal persistence failure.
fn map_persist(error: sqlx::Error) -> EnrollmentError {
    if error
        .as_database_error()
        .is_some_and(|db| db.is_unique_violation())
    {
        return EnrollmentError::Conflict;
    }
    EnrollmentError::Persistence
}

/// Mask an API key to a non-secret identity hint (last four characters).
fn mask_key(value: &str) -> String {
    let suffix = value.chars().rev().take(4).collect::<String>();
    format!("****{}", suffix.chars().rev().collect::<String>())
}
