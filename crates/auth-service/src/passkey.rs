//! WebAuthn passkeys and one-use action-authorization proofs (M4 security slice).
//!
//! This module is the credential-authorization primitive that later M4 slices
//! (credential enrollment, account arming) build on. It owns the WebAuthn
//! ceremonies and the persistence of registered authenticators, pending
//! challenges, and consumed action proofs. The anti-clone and single-use
//! invariants live in [`funding_arb_domain::auth`]; the cryptographic
//! verification is delegated to `webauthn-rs`.
//!
//! Every table is tenant-owned and forced-RLS (migration `0017`), so all
//! persistence flows through [`TenantTransaction`]: a request that presents no
//! tenant context reads and writes nothing. Ceremony state and the serialized
//! authenticator are opaque blobs; no public key material or challenge is ever
//! returned outside its ceremony.

use chrono::{Duration, Utc};
use funding_arb_domain::auth::{ActionDescriptor, CredentialId, PasskeyCredential, SignCount};
use funding_arb_tenant_context::{TenantContext, TenantTransaction};
use rand::{RngCore, rngs::OsRng};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Url, Webauthn, WebauthnBuilder,
};

use crate::error::{AuthError, AuthResult};
use crate::service::CallerContext;

/// Ceremonies expire quickly; a passkey exchange is interactive and immediate.
const CHALLENGE_TTL_SECONDS: i64 = 300;

const CEREMONY_REGISTER: &str = "REGISTER";
const CEREMONY_AUTHENTICATE: &str = "AUTHENTICATE";

/// Build a relying-party `Webauthn` instance from configuration.
///
/// # Errors
/// Returns [`ConfigError`](crate::config::ConfigError) when an origin is not a
/// valid URL or the builder rejects the relying-party parameters.
pub fn build_webauthn(
    config: &crate::config::PasskeyConfig,
) -> Result<Webauthn, crate::config::ConfigError> {
    use crate::config::ConfigError;

    let mut origins = config.origins.iter();
    let primary = origins
        .next()
        .ok_or_else(|| ConfigError("at least one passkey origin is required".to_owned()))?;
    let primary_url = Url::parse(primary)
        .map_err(|error| ConfigError(format!("invalid passkey origin {primary}: {error}")))?;
    let mut builder = WebauthnBuilder::new(&config.rp_id, &primary_url)
        .map_err(|error| ConfigError(format!("invalid passkey relying party: {error}")))?
        .rp_name(&config.rp_name);
    for origin in origins {
        let url = Url::parse(origin)
            .map_err(|error| ConfigError(format!("invalid passkey origin {origin}: {error}")))?;
        builder = builder.append_allowed_origin(&url);
    }
    builder
        .build()
        .map_err(|error| ConfigError(format!("failed to build passkey relying party: {error}")))
}

/// A non-secret view of a registered passkey.
#[derive(Clone, Debug, Serialize)]
pub struct PasskeyView {
    pub id: String,
    pub label: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

/// The challenge handed to the client to start a ceremony.
#[derive(Clone, Debug, Serialize)]
pub struct CeremonyChallenge {
    /// Correlates the client's later `finish` call to the stored ceremony state.
    pub challenge_id: String,
    /// The opaque `navigator.credentials` options for the browser/authenticator.
    pub options: Value,
}

/// Outcome of a bare assertion (identity re-proof, no bound action).
#[derive(Clone, Debug, Serialize)]
pub struct AssertionOutcome {
    pub verified: bool,
    pub credential_id: String,
}

/// Receipt for a consumed one-use action proof.
#[derive(Clone, Debug, Serialize)]
pub struct ActionProofReceipt {
    pub proof_id: String,
    pub action: String,
    pub credential_id: String,
}

/// WebAuthn application service. Cheap to clone (shares the pool and RP).
#[derive(Clone)]
pub struct PasskeyService {
    pool: PgPool,
    webauthn: std::sync::Arc<Webauthn>,
}

impl PasskeyService {
    #[must_use]
    pub fn new(pool: PgPool, webauthn: Webauthn) -> Self {
        Self {
            pool,
            webauthn: std::sync::Arc::new(webauthn),
        }
    }

    fn user_uuid(caller: &CallerContext) -> AuthResult<Uuid> {
        caller
            .user_id
            .parse::<Uuid>()
            .map_err(|_| AuthError::Unauthorized)
    }

    // ----- Registration -----------------------------------------------------

    /// Begin registering a new authenticator for the caller.
    pub async fn start_registration(
        &self,
        caller: &CallerContext,
    ) -> AuthResult<CeremonyChallenge> {
        let tenant = caller.tenant_context()?;
        let user_id = Self::user_uuid(caller)?;
        // Exclude already-registered credentials so the user cannot double-register
        // the same authenticator.
        let existing = self.load_active_passkeys(tenant, user_id).await?;
        let exclude = existing
            .iter()
            .map(|(_, passkey)| passkey.cred_id().clone())
            .collect();

        let (options, state) = self
            .webauthn
            .start_passkey_registration(user_id, &caller.user_id, &caller.user_id, Some(exclude))
            .map_err(|error| AuthError::Internal(format!("passkey registration start: {error}")))?;

        let state_bytes = serde_json::to_vec(&state).map_err(|error| {
            AuthError::Internal(format!("serialize registration state: {error}"))
        })?;
        let challenge_id = self
            .persist_challenge(tenant, user_id, CEREMONY_REGISTER, &state_bytes, None)
            .await?;
        Ok(CeremonyChallenge {
            challenge_id: challenge_id.to_string(),
            options: to_json(&options)?,
        })
    }

    /// Finish registration, storing the verified authenticator under `label`.
    pub async fn finish_registration(
        &self,
        caller: &CallerContext,
        challenge_id: &str,
        label: &str,
        credential: Value,
    ) -> AuthResult<PasskeyView> {
        let tenant = caller.tenant_context()?;
        let user_id = Self::user_uuid(caller)?;
        let label = validate_label(label)?;
        let challenge_id = parse_uuid(challenge_id)?;

        let mut transaction = self.begin(tenant).await?;
        let state_bytes = consume_challenge(
            &mut transaction,
            tenant,
            user_id,
            challenge_id,
            CEREMONY_REGISTER,
        )
        .await?;
        let state: PasskeyRegistration = serde_json::from_slice(&state_bytes).map_err(|error| {
            AuthError::Internal(format!("deserialize registration state: {error}"))
        })?;
        let register: RegisterPublicKeyCredential = serde_json::from_value(credential)
            .map_err(|_| AuthError::BadRequest("malformed registration credential"))?;

        let passkey = self
            .webauthn
            .finish_passkey_registration(&register, &state)
            .map_err(|_| AuthError::PasskeyProofInvalid)?;

        let credential_id = passkey.cred_id().as_ref().to_vec();
        let authenticator = serde_json::to_vec(&passkey)
            .map_err(|error| AuthError::Internal(format!("serialize authenticator: {error}")))?;

        let id = Uuid::new_v4();
        let created_at: chrono::DateTime<Utc> = sqlx::query_scalar(
            "INSERT INTO passkey_credentials
                 (id, tenant_id, user_id, credential_id, authenticator, sign_count, label)
             VALUES ($1, $2, $3, $4, $5, 0, $6)
             RETURNING created_at",
        )
        .bind(id)
        .bind(tenant.tenant_id().as_uuid())
        .bind(user_id)
        .bind(credential_id.as_slice())
        .bind(authenticator.as_slice())
        .bind(&label)
        .fetch_one(transaction.connection())
        .await
        .map_err(map_unique_conflict)?;
        transaction.commit().await?;

        Ok(PasskeyView {
            id: id.to_string(),
            label,
            created_at: created_at.to_rfc3339(),
            last_used_at: None,
        })
    }

    // ----- Assertion --------------------------------------------------------

    /// Begin a bare identity assertion against the caller's passkeys.
    pub async fn start_authentication(
        &self,
        caller: &CallerContext,
    ) -> AuthResult<CeremonyChallenge> {
        self.start_assertion(caller, None).await
    }

    /// Finish a bare identity assertion (no bound action).
    pub async fn finish_authentication(
        &self,
        caller: &CallerContext,
        challenge_id: &str,
        credential: Value,
    ) -> AuthResult<AssertionOutcome> {
        let tenant = caller.tenant_context()?;
        let user_id = Self::user_uuid(caller)?;
        let challenge_id = parse_uuid(challenge_id)?;

        let mut transaction = self.begin(tenant).await?;
        let (state_bytes, _) =
            consume_assertion_challenge(&mut transaction, tenant, user_id, challenge_id).await?;
        let credential_id = self
            .verify_and_advance(&mut transaction, tenant, &state_bytes, credential)
            .await?;
        transaction.commit().await?;
        Ok(AssertionOutcome {
            verified: true,
            credential_id: hex(&credential_id),
        })
    }

    // ----- Action authorization ---------------------------------------------

    /// Begin an assertion that will authorize exactly `action`.
    pub async fn start_action(
        &self,
        caller: &CallerContext,
        action: &ActionDescriptor,
    ) -> AuthResult<CeremonyChallenge> {
        self.start_assertion(caller, Some(action)).await
    }

    /// Finish an action-bound assertion, recording a single-use proof.
    ///
    /// The proof is bound to `action`: the stored challenge's descriptor must
    /// match, and the `(tenant, challenge)` uniqueness on `passkey_action_proofs`
    /// makes any replay of the same verified assertion fail closed.
    pub async fn finish_action(
        &self,
        caller: &CallerContext,
        challenge_id: &str,
        action: &ActionDescriptor,
        credential: Value,
    ) -> AuthResult<ActionProofReceipt> {
        let tenant = caller.tenant_context()?;
        let user_id = Self::user_uuid(caller)?;
        let challenge_id = parse_uuid(challenge_id)?;

        let mut transaction = self.begin(tenant).await?;
        let (state_bytes, bound) =
            consume_assertion_challenge(&mut transaction, tenant, user_id, challenge_id).await?;
        // The proof may only authorize the exact action the challenge was minted
        // for. A mismatch (or a challenge minted without a bound action) fails.
        match bound {
            Some(descriptor) if descriptor == action.canonical() => {}
            _ => return Err(AuthError::PasskeyProofInvalid),
        }
        let credential_id = self
            .verify_and_advance(&mut transaction, tenant, &state_bytes, credential)
            .await?;

        let proof_id = Uuid::new_v4();
        let nonce = challenge_id; // the consumed challenge id is the unique replay key
        sqlx::query(
            "INSERT INTO passkey_action_proofs
                 (id, tenant_id, credential_id, action_descriptor, challenge)
             VALUES ($1, $2, $3, $4::jsonb, $5)",
        )
        .bind(proof_id)
        .bind(tenant.tenant_id().as_uuid())
        .bind(credential_id.as_slice())
        .bind(descriptor_json(action))
        .bind(nonce.as_bytes().as_slice())
        .execute(transaction.connection())
        .await
        .map_err(map_unique_conflict)?;
        transaction.commit().await?;

        Ok(ActionProofReceipt {
            proof_id: proof_id.to_string(),
            action: action.canonical(),
            credential_id: hex(&credential_id),
        })
    }

    // ----- Management -------------------------------------------------------

    /// List the caller's active passkeys.
    pub async fn list(&self, caller: &CallerContext) -> AuthResult<Vec<PasskeyView>> {
        let tenant = caller.tenant_context()?;
        let user_id = Self::user_uuid(caller)?;
        let mut transaction = self.begin(tenant).await?;
        let rows = sqlx::query(
            "SELECT id::text AS id, label, created_at, last_used_at
             FROM passkey_credentials
             WHERE tenant_id = $1 AND user_id = $2 AND revoked_at IS NULL
             ORDER BY created_at DESC",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(user_id)
        .fetch_all(transaction.connection())
        .await?;
        transaction.commit().await?;
        Ok(rows
            .into_iter()
            .map(|row| PasskeyView {
                id: row.get("id"),
                label: row.get("label"),
                created_at: row
                    .get::<chrono::DateTime<Utc>, _>("created_at")
                    .to_rfc3339(),
                last_used_at: row
                    .get::<Option<chrono::DateTime<Utc>>, _>("last_used_at")
                    .map(|value| value.to_rfc3339()),
            })
            .collect())
    }

    /// Revoke one of the caller's passkeys by row id.
    pub async fn revoke(&self, caller: &CallerContext, passkey_id: &str) -> AuthResult<()> {
        let tenant = caller.tenant_context()?;
        let user_id = Self::user_uuid(caller)?;
        let passkey_id = parse_uuid(passkey_id)?;
        let mut transaction = self.begin(tenant).await?;
        let affected = sqlx::query(
            "UPDATE passkey_credentials SET revoked_at = now()
             WHERE tenant_id = $1 AND user_id = $2 AND id = $3 AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(user_id)
        .bind(passkey_id)
        .execute(transaction.connection())
        .await?
        .rows_affected();
        transaction.commit().await?;
        if affected == 0 {
            return Err(AuthError::NotFound);
        }
        Ok(())
    }

    // ----- Internals --------------------------------------------------------

    async fn begin(&self, tenant: TenantContext) -> AuthResult<TenantTransaction<'_>> {
        TenantTransaction::begin(&self.pool, tenant)
            .await
            .map_err(|error| AuthError::Internal(format!("tenant transaction: {error}")))
    }

    async fn start_assertion(
        &self,
        caller: &CallerContext,
        action: Option<&ActionDescriptor>,
    ) -> AuthResult<CeremonyChallenge> {
        let tenant = caller.tenant_context()?;
        let user_id = Self::user_uuid(caller)?;
        let passkeys: Vec<Passkey> = self
            .load_active_passkeys(tenant, user_id)
            .await?
            .into_iter()
            .map(|(_, passkey)| passkey)
            .collect();
        if passkeys.is_empty() {
            return Err(AuthError::NotFound);
        }
        let (options, state) = self
            .webauthn
            .start_passkey_authentication(&passkeys)
            .map_err(|error| AuthError::Internal(format!("passkey assertion start: {error}")))?;
        let state_bytes = serde_json::to_vec(&state)
            .map_err(|error| AuthError::Internal(format!("serialize assertion state: {error}")))?;
        let bound = action.map(descriptor_json);
        let challenge_id = self
            .persist_challenge(tenant, user_id, CEREMONY_AUTHENTICATE, &state_bytes, bound)
            .await?;
        Ok(CeremonyChallenge {
            challenge_id: challenge_id.to_string(),
            options: to_json(&options)?,
        })
    }

    /// Verify an assertion, enforce the domain anti-clone invariant, and persist
    /// the advanced authenticator. Returns the asserted credential id.
    async fn verify_and_advance(
        &self,
        transaction: &mut TenantTransaction<'_>,
        tenant: TenantContext,
        state_bytes: &[u8],
        credential: Value,
    ) -> AuthResult<Vec<u8>> {
        let state: PasskeyAuthentication =
            serde_json::from_slice(state_bytes).map_err(|error| {
                AuthError::Internal(format!("deserialize assertion state: {error}"))
            })?;
        let assertion: PublicKeyCredential = serde_json::from_value(credential)
            .map_err(|_| AuthError::BadRequest("malformed assertion credential"))?;
        let result = self
            .webauthn
            .finish_passkey_authentication(&assertion, &state)
            .map_err(|_| AuthError::PasskeyProofInvalid)?;
        let credential_id = result.cred_id().as_ref().to_vec();

        // Load the stored authenticator under RLS and apply the monotonic
        // signature-count invariant in the domain before trusting the update.
        let row = sqlx::query(
            "SELECT authenticator, sign_count FROM passkey_credentials
             WHERE tenant_id = $1 AND credential_id = $2 AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(credential_id.as_slice())
        .fetch_optional(transaction.connection())
        .await?
        .ok_or(AuthError::PasskeyProofInvalid)?;
        let stored_count: i64 = row.get("sign_count");
        let mut authenticator: Passkey =
            serde_json::from_slice(&row.get::<Vec<u8>, _>("authenticator")).map_err(|error| {
                AuthError::Internal(format!("deserialize authenticator: {error}"))
            })?;

        let mut credential = PasskeyCredential::new(
            CredentialId::new(credential_id.clone()),
            SignCount::new(u32::try_from(stored_count).unwrap_or(u32::MAX)),
        );
        credential
            .record_assertion(SignCount::new(result.counter()))
            .map_err(|_| AuthError::PasskeyProofInvalid)?;

        // webauthn-rs updates the counter and backup state inside the authenticator.
        authenticator.update_credential(&result);
        let updated = serde_json::to_vec(&authenticator)
            .map_err(|error| AuthError::Internal(format!("serialize authenticator: {error}")))?;
        sqlx::query(
            "UPDATE passkey_credentials
             SET authenticator = $3, sign_count = $4, last_used_at = now()
             WHERE tenant_id = $1 AND credential_id = $2",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(credential_id.as_slice())
        .bind(updated.as_slice())
        .bind(i64::from(result.counter()))
        .execute(transaction.connection())
        .await?;
        Ok(credential_id)
    }

    async fn load_active_passkeys(
        &self,
        tenant: TenantContext,
        user_id: Uuid,
    ) -> AuthResult<Vec<(Uuid, Passkey)>> {
        let mut transaction = self.begin(tenant).await?;
        let rows = sqlx::query(
            "SELECT id, authenticator FROM passkey_credentials
             WHERE tenant_id = $1 AND user_id = $2 AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(user_id)
        .fetch_all(transaction.connection())
        .await?;
        transaction.commit().await?;
        let mut passkeys = Vec::with_capacity(rows.len());
        for row in rows {
            let passkey: Passkey = serde_json::from_slice(&row.get::<Vec<u8>, _>("authenticator"))
                .map_err(|error| {
                    AuthError::Internal(format!("deserialize authenticator: {error}"))
                })?;
            passkeys.push((row.get::<Uuid, _>("id"), passkey));
        }
        Ok(passkeys)
    }

    async fn persist_challenge(
        &self,
        tenant: TenantContext,
        user_id: Uuid,
        ceremony: &str,
        state_bytes: &[u8],
        action_descriptor: Option<String>,
    ) -> AuthResult<Uuid> {
        let mut transaction = self.begin(tenant).await?;
        // Clear any prior live ceremony of the same kind (enforced by a partial
        // unique index) so a fresh ceremony always supersedes an abandoned one.
        sqlx::query(
            "DELETE FROM webauthn_challenges
             WHERE tenant_id = $1 AND user_id = $2 AND ceremony = $3::webauthn_ceremony
               AND consumed_at IS NULL",
        )
        .bind(tenant.tenant_id().as_uuid())
        .bind(user_id)
        .bind(ceremony)
        .execute(transaction.connection())
        .await?;

        let id = Uuid::new_v4();
        let mut nonce = [0_u8; 32];
        OsRng.fill_bytes(&mut nonce);
        let expires_at = Utc::now() + Duration::seconds(CHALLENGE_TTL_SECONDS);
        sqlx::query(
            "INSERT INTO webauthn_challenges
                 (id, tenant_id, user_id, ceremony, state, challenge, action_descriptor, expires_at)
             VALUES ($1, $2, $3, $4::webauthn_ceremony, $5, $6, $7::jsonb, $8)",
        )
        .bind(id)
        .bind(tenant.tenant_id().as_uuid())
        .bind(user_id)
        .bind(ceremony)
        .bind(state_bytes)
        .bind(nonce.as_slice())
        .bind(action_descriptor)
        .bind(expires_at)
        .execute(transaction.connection())
        .await?;
        transaction.commit().await?;
        Ok(id)
    }
}

/// Consume a pending challenge of a given ceremony, returning its state. Fails
/// closed on a missing, expired, already-consumed, or wrong-ceremony challenge.
async fn consume_challenge(
    transaction: &mut TenantTransaction<'_>,
    tenant: TenantContext,
    user_id: Uuid,
    challenge_id: Uuid,
    ceremony: &str,
) -> AuthResult<Vec<u8>> {
    let (state, _) =
        consume_challenge_inner(transaction, tenant, user_id, challenge_id, Some(ceremony)).await?;
    Ok(state)
}

/// Consume a pending assertion challenge, returning its state and bound action.
async fn consume_assertion_challenge(
    transaction: &mut TenantTransaction<'_>,
    tenant: TenantContext,
    user_id: Uuid,
    challenge_id: Uuid,
) -> AuthResult<(Vec<u8>, Option<String>)> {
    consume_challenge_inner(
        transaction,
        tenant,
        user_id,
        challenge_id,
        Some(CEREMONY_AUTHENTICATE),
    )
    .await
}

async fn consume_challenge_inner(
    transaction: &mut TenantTransaction<'_>,
    tenant: TenantContext,
    user_id: Uuid,
    challenge_id: Uuid,
    ceremony: Option<&str>,
) -> AuthResult<(Vec<u8>, Option<String>)> {
    let row = sqlx::query(
        "UPDATE webauthn_challenges SET consumed_at = now()
         WHERE tenant_id = $1 AND user_id = $2 AND id = $3
           AND ceremony = $4::webauthn_ceremony
           AND consumed_at IS NULL AND expires_at > now()
         RETURNING state, action_descriptor::text AS action_descriptor",
    )
    .bind(tenant.tenant_id().as_uuid())
    .bind(user_id)
    .bind(challenge_id)
    .bind(ceremony.expect("ceremony is always supplied"))
    .fetch_optional(transaction.connection())
    .await?
    .ok_or(AuthError::ChallengeConsumed)?;
    let state: Vec<u8> = row.get("state");
    let action: Option<String> = row
        .get::<Option<String>, _>("action_descriptor")
        .and_then(parse_action_canonical);
    Ok((state, action))
}

fn descriptor_json(action: &ActionDescriptor) -> String {
    // Canonical form persisted as a small stable JSON object.
    serde_json::json!({ "action": action.canonical() }).to_string()
}

fn parse_action_canonical(raw: String) -> Option<String> {
    serde_json::from_str::<Value>(&raw).ok().and_then(|value| {
        value
            .get("action")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
}

fn to_json<T: Serialize>(value: &T) -> AuthResult<Value> {
    serde_json::to_value(value)
        .map_err(|error| AuthError::Internal(format!("serialize ceremony options: {error}")))
}

fn validate_label(label: &str) -> AuthResult<String> {
    let trimmed = label.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 120 {
        return Err(AuthError::BadRequest(
            "passkey label must be 1-120 characters",
        ));
    }
    Ok(trimmed.to_owned())
}

fn parse_uuid(value: &str) -> AuthResult<Uuid> {
    value
        .parse::<Uuid>()
        .map_err(|_| AuthError::BadRequest("invalid identifier"))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Map a Postgres unique-violation to a fail-closed conflict; anything else is
/// an internal error.
fn map_unique_conflict(error: sqlx::Error) -> AuthError {
    if error
        .as_database_error()
        .is_some_and(|db| db.is_unique_violation())
    {
        return AuthError::PasskeyProofInvalid;
    }
    AuthError::Internal(format!("database error: {error}"))
}
