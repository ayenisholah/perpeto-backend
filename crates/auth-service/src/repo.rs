//! Transactional Postgres persistence for authentication.
//!
//! Enum columns are cast to/from `text` so the driver needs no compile-time
//! schema, keeping the crate buildable without a live database.

use chrono::{DateTime, Duration, SubsecRound, Utc};
use funding_arb_tenant_context::{TenantContext, TenantTransaction};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::audit::{canonical, event_hash};
use crate::error::{AuthError, AuthResult};

const SESSION_TTL_DAYS: i64 = 30;

/// A read projection of a user assembled from several tables.
#[derive(Clone, Debug)]
pub struct UserProjection {
    pub id: String,
    pub tenant_id: String,
    pub tenant_display_name: String,
    pub tenant_base_currency: String,
    pub tenant_state: String,
    pub status: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub permission_version: i64,
    pub roles: Vec<String>,
    pub providers: Vec<String>,
    pub mfa_enrolled: bool,
}

/// The parsed challenge row needed for verification.
#[derive(Clone, Debug)]
pub struct ChallengeRow {
    pub provider: String,
    pub purpose: String,
    pub nonce_hash: Vec<u8>,
    pub pkce_challenge: String,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

/// A verified identity ready for persistence.
#[derive(Clone, Debug)]
pub struct IdentityUpsert {
    pub provider: String,
    pub subject: String,
    pub email: Option<String>,
    pub is_private_relay: bool,
    pub display_name: Option<String>,
}

/// Device fingerprint for session binding.
#[derive(Clone, Debug)]
pub struct DeviceUpsert {
    pub installation_id_hash: Vec<u8>,
    pub app_version: String,
    pub display_name: String,
}

/// Outcome of a successful exchange transaction.
#[derive(Clone, Debug)]
pub struct ExchangeOutcome {
    pub user: UserProjection,
    pub device_id: String,
}

/// Thin data-access layer over a Postgres pool.
#[derive(Clone)]
pub struct Repo {
    pool: PgPool,
}

impl Repo {
    /// Wrap a connection pool.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Verify the pool can service a trivial query (readiness probe).
    ///
    /// # Errors
    /// Propagates any connection failure.
    pub async fn ping(&self) -> AuthResult<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Read a tenant's `tenant_state` text form.
    ///
    /// # Errors
    /// [`AuthError::NotFound`] when the tenant does not exist; database failures
    /// otherwise.
    pub async fn pilot_state(&self, tenant_id: &str) -> AuthResult<String> {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state::text FROM tenants WHERE id = $1")
                .bind(parse_uuid(tenant_id)?)
                .fetch_optional(&self.pool)
                .await?;
        state.ok_or(AuthError::NotFound)
    }

    /// Transition a tenant's `tenant_state`. Deleted tenants are immutable.
    ///
    /// # Errors
    /// [`AuthError::NotFound`] when no live tenant row matched; database failures
    /// otherwise.
    pub async fn set_pilot_state(&self, tenant_id: &str, state: &str) -> AuthResult<()> {
        let affected = sqlx::query(
            "UPDATE tenants SET state = $2::tenant_state, updated_at = now() \
             WHERE id = $1 AND state <> 'DELETED'",
        )
        .bind(parse_uuid(tenant_id)?)
        .bind(state)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected == 0 {
            return Err(AuthError::NotFound);
        }
        Ok(())
    }

    /// Insert a fresh sign-in or link challenge.
    ///
    /// # Errors
    /// Propagates database failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_challenge(
        &self,
        provider: &str,
        purpose: &str,
        user_id: Option<&str>,
        state_hash: &[u8],
        nonce_hash: &[u8],
        pkce_challenge: &str,
        redirect_uri: &str,
        ttl_seconds: i64,
    ) -> AuthResult<String> {
        let id = Uuid::new_v4();
        let expires_at = Utc::now() + Duration::seconds(ttl_seconds);
        sqlx::query(
            "INSERT INTO social_challenges \
             (id, purpose, provider, user_id, state_hash, nonce_hash, pkce_challenge, redirect_uri, expires_at) \
             VALUES ($1, $2::auth_challenge_purpose, $3::social_provider, $4::uuid, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(purpose)
        .bind(provider)
        .bind(user_id)
        .bind(state_hash)
        .bind(nonce_hash)
        .bind(pkce_challenge)
        .bind(redirect_uri)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(id.to_string())
    }

    /// Load a challenge for pre-transaction verification.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn load_challenge(&self, id: &str) -> AuthResult<Option<ChallengeRow>> {
        let uuid = parse_uuid(id)?;
        let row = sqlx::query(
            "SELECT provider::text AS provider, purpose::text AS purpose, nonce_hash, \
             pkce_challenge, expires_at, consumed_at \
             FROM social_challenges WHERE id = $1",
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| ChallengeRow {
            provider: row.get("provider"),
            purpose: row.get("purpose"),
            nonce_hash: row.get("nonce_hash"),
            pkce_challenge: row.get("pkce_challenge"),
            expires_at: row.get("expires_at"),
            consumed_at: row.get("consumed_at"),
        }))
    }

    /// Consume a challenge and upsert the identity, user, device, and session in
    /// a single transaction.
    ///
    /// # Errors
    /// Returns [`AuthError::ChallengeConsumed`] if the challenge is spent or
    /// expired, or propagates database failures.
    pub async fn commit_exchange(
        &self,
        challenge_id: &str,
        identity: &IdentityUpsert,
        device: &DeviceUpsert,
        session_id: Option<&str>,
        refresh_token_hash: Option<&[u8]>,
        auto_approve: bool,
    ) -> AuthResult<ExchangeOutcome> {
        let challenge_uuid = parse_uuid(challenge_id)?;
        let mut tx = self.pool.begin().await?;

        let locked = sqlx::query(
            "SELECT expires_at, consumed_at FROM social_challenges WHERE id = $1 FOR UPDATE",
        )
        .bind(challenge_uuid)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AuthError::ChallengeConsumed)?;
        let consumed_at: Option<DateTime<Utc>> = locked.get("consumed_at");
        let expires_at: DateTime<Utc> = locked.get("expires_at");
        if consumed_at.is_some() || expires_at <= Utc::now() {
            return Err(AuthError::ChallengeConsumed);
        }
        sqlx::query("UPDATE social_challenges SET consumed_at = now() WHERE id = $1")
            .bind(challenge_uuid)
            .execute(&mut *tx)
            .await?;

        let existing = sqlx::query(
            "SELECT pi.user_id::text AS user_id, u.status::text AS status \
             FROM provider_identities pi JOIN users u ON u.id = pi.user_id \
             WHERE pi.provider = $1::social_provider AND pi.provider_subject = $2",
        )
        .bind(&identity.provider)
        .bind(&identity.subject)
        .fetch_optional(&mut *tx)
        .await?;
        let existing_user: Option<(String, String)> =
            existing.map(|row| (row.get("user_id"), row.get("status")));

        let user_id = match existing_user {
            // A live account keeps its identity row; refresh the mutable profile.
            Some((user_id, status)) if status != "DELETED" => {
                sqlx::query(
                    "UPDATE provider_identities \
                     SET last_authenticated_at = now(), profile_email = $3, \
                         profile_is_private_relay = $4 \
                     WHERE provider = $1::social_provider AND provider_subject = $2",
                )
                .bind(&identity.provider)
                .bind(&identity.subject)
                .bind(&identity.email)
                .bind(identity.is_private_relay)
                .execute(&mut *tx)
                .await?;
                user_id
            }
            // The identity was previously deleted: register a fresh account and
            // re-point the (unique) identity row to it, clearing the revocation.
            Some(_deleted) => {
                let new_user = insert_new_user(&mut tx, identity, auto_approve).await?;
                sqlx::query(
                    "UPDATE provider_identities \
                     SET user_id = $3, revoked_at = NULL, linked_at = now(), \
                         last_authenticated_at = now(), profile_email = $4, \
                         profile_is_private_relay = $5, profile_display_name = $6 \
                     WHERE provider = $1::social_provider AND provider_subject = $2",
                )
                .bind(&identity.provider)
                .bind(&identity.subject)
                .bind(new_user)
                .bind(&identity.email)
                .bind(identity.is_private_relay)
                .bind(&identity.display_name)
                .execute(&mut *tx)
                .await?;
                new_user.to_string()
            }
            None => {
                let new_user = insert_new_user(&mut tx, identity, auto_approve).await?;
                sqlx::query(
                    "INSERT INTO provider_identities \
                     (id, user_id, provider, provider_subject, profile_email, \
                      profile_is_private_relay, profile_display_name) \
                     VALUES ($1, $2, $3::social_provider, $4, $5, $6, $7)",
                )
                .bind(Uuid::new_v4())
                .bind(new_user)
                .bind(&identity.provider)
                .bind(&identity.subject)
                .bind(&identity.email)
                .bind(identity.is_private_relay)
                .bind(&identity.display_name)
                .execute(&mut *tx)
                .await?;
                new_user.to_string()
            }
        };

        let user_uuid = parse_uuid(&user_id)?;
        let device_id: String = sqlx::query_scalar(
            "INSERT INTO devices \
             (id, user_id, installation_id_hash, platform, app_version, display_name) \
             VALUES ($1, $2, $3, 'IOS', $4, $5) \
             ON CONFLICT (user_id, installation_id_hash) DO UPDATE \
             SET last_seen_at = now(), app_version = EXCLUDED.app_version, \
                 display_name = EXCLUDED.display_name, revoked_at = NULL \
             RETURNING id::text",
        )
        .bind(Uuid::new_v4())
        .bind(user_uuid)
        .bind(&device.installation_id_hash)
        .bind(&device.app_version)
        .bind(&device.display_name)
        .fetch_one(&mut *tx)
        .await?;

        let user = load_user_projection(&mut tx, &user_id).await?;

        if let (Some(session_id), Some(refresh_hash)) = (session_id, refresh_token_hash) {
            insert_session(
                &mut *tx,
                session_id,
                &user_id,
                &device_id,
                refresh_hash,
                user.permission_version,
            )
            .await?;
        }

        tx.commit().await?;
        Ok(ExchangeOutcome { user, device_id })
    }

    /// Load the public projection of a user.
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] if the user does not exist.
    pub async fn user_projection(&self, user_id: &str) -> AuthResult<UserProjection> {
        let mut conn = self.pool.acquire().await?;
        load_user_projection(&mut conn, user_id).await
    }

    /// Find an active session by its refresh-token hash.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn session_by_refresh_hash(&self, hash: &[u8]) -> AuthResult<Option<SessionRow>> {
        let row = sqlx::query(
            "SELECT id::text AS id, user_id::text AS user_id, tenant_id::text AS tenant_id, \
             device_id::text AS device_id, state::text AS state, generation, permission_version, expires_at \
             FROM sessions WHERE refresh_token_hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| SessionRow {
            id: row.get("id"),
            user_id: row.get("user_id"),
            tenant_id: row.get("tenant_id"),
            device_id: row.get("device_id"),
            state: row.get("state"),
            generation: row.get("generation"),
            permission_version: row.get("permission_version"),
            expires_at: row.get("expires_at"),
        }))
    }

    /// Return the session id that once held this now-replayed token, if any.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn session_for_historic_token(&self, hash: &[u8]) -> AuthResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT session_id::text FROM refresh_token_history WHERE token_hash = $1",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Mark a session compromised after a refresh-token replay.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn compromise_session(&self, session_id: &str) -> AuthResult<()> {
        let uuid = parse_uuid(session_id)?;
        sqlx::query(
            "UPDATE sessions SET state = 'COMPROMISED', revoked_at = now(), \
             revoke_reason = 'REFRESH_TOKEN_REUSE' WHERE id = $1 AND state = 'ACTIVE'",
        )
        .bind(uuid)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Revoke a session (permission-version drift).
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn revoke_session_internal(&self, session_id: &str, reason: &str) -> AuthResult<()> {
        let uuid = parse_uuid(session_id)?;
        sqlx::query(
            "UPDATE sessions SET state = 'REVOKED', revoked_at = now(), revoke_reason = $2 \
             WHERE id = $1 AND state = 'ACTIVE'",
        )
        .bind(uuid)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Rotate a session's refresh token, archiving the presented one.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn rotate_refresh(
        &self,
        session_id: &str,
        old_hash: &[u8],
        old_generation: i64,
        new_hash: &[u8],
    ) -> AuthResult<()> {
        let uuid = parse_uuid(session_id)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO refresh_token_history (session_id, token_hash, generation) \
             VALUES ($1, $2, $3)",
        )
        .bind(uuid)
        .bind(old_hash)
        .bind(old_generation)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sessions SET refresh_token_hash = $2, generation = generation + 1, \
             last_rotated_at = now() WHERE id = $1",
        )
        .bind(uuid)
        .bind(new_hash)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Create a fresh session for a user + device (used by the MFA path).
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn create_session(
        &self,
        session_id: &str,
        user_id: &str,
        device_id: &str,
        refresh_hash: &[u8],
        permission_version: i64,
    ) -> AuthResult<()> {
        let mut conn = self.pool.acquire().await?;
        insert_session(
            &mut *conn,
            session_id,
            user_id,
            device_id,
            refresh_hash,
            permission_version,
        )
        .await
    }

    /// Load the deployment bootstrap state.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn bootstrap_state(&self) -> AuthResult<Option<BootstrapRow>> {
        let row = sqlx::query(
            "SELECT deployment_token_hash, expires_at, consumed_at \
             FROM bootstrap_state WHERE singleton = true",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| BootstrapRow {
            deployment_token_hash: row.get("deployment_token_hash"),
            expires_at: row.get("expires_at"),
            consumed_at: row.get("consumed_at"),
        }))
    }

    /// Promote a user to active Owner and consume the bootstrap token.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn commit_bootstrap(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> AuthResult<UserProjection> {
        let user_uuid = parse_uuid(user_id)?;
        let session_uuid = parse_uuid(session_id)?;
        let mut tx = self.pool.begin().await?;
        let permission_version: i64 = sqlx::query_scalar(
            "UPDATE users SET status = 'ACTIVE', permission_version = permission_version + 1, \
             updated_at = now() WHERE id = $1 RETURNING permission_version",
        )
        .bind(user_uuid)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO user_roles (user_id, role, active, activated_at) \
             VALUES ($1, 'OWNER', true, now()) \
             ON CONFLICT (user_id, role) DO UPDATE SET active = true, activated_at = now()",
        )
        .bind(user_uuid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE bootstrap_state SET consumed_by = $1, consumed_at = now() \
             WHERE singleton = true",
        )
        .bind(user_uuid)
        .execute(&mut *tx)
        .await?;
        // Keep the caller's current session valid after the permission bump.
        sqlx::query("UPDATE sessions SET permission_version = $2 WHERE id = $1")
            .bind(session_uuid)
            .bind(permission_version)
            .execute(&mut *tx)
            .await?;
        let user = load_user_projection(&mut tx, user_id).await?;
        tx.commit().await?;
        Ok(user)
    }

    /// Revoke the given active session (logout).
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn logout(&self, session_id: &str) -> AuthResult<()> {
        self.revoke_session_internal(session_id, "LOGOUT").await
    }

    /// List a user's active sessions with their device names.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn list_sessions(&self, user_id: &str) -> AuthResult<Vec<SessionSummaryRow>> {
        let uuid = parse_uuid(user_id)?;
        let rows = sqlx::query(
            "SELECT s.id::text AS id, s.device_id::text AS device_id, d.display_name AS device_name, \
             s.created_at, d.last_seen_at \
             FROM sessions s JOIN devices d ON d.id = s.device_id \
             WHERE s.user_id = $1 AND s.state = 'ACTIVE' ORDER BY s.created_at DESC",
        )
        .bind(uuid)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| SessionSummaryRow {
                id: row.get("id"),
                device_id: row.get("device_id"),
                device_name: row.get("device_name"),
                created_at: row.get("created_at"),
                last_seen_at: row.get("last_seen_at"),
            })
            .collect())
    }

    /// Revoke one of the user's sessions by id.
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] if no matching active session exists.
    pub async fn revoke_own_session(&self, user_id: &str, session_id: &str) -> AuthResult<()> {
        let user_uuid = parse_uuid(user_id)?;
        let session_uuid = parse_uuid(session_id)?;
        let result = sqlx::query(
            "UPDATE sessions SET state = 'REVOKED', revoked_at = now(), \
             revoke_reason = 'USER_REVOKED' \
             WHERE id = $1 AND user_id = $2 AND state = 'ACTIVE'",
        )
        .bind(session_uuid)
        .bind(user_uuid)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(AuthError::NotFound);
        }
        Ok(())
    }

    /// List a user's linked identities.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn list_identities(&self, user_id: &str) -> AuthResult<Vec<IdentityRow>> {
        let uuid = parse_uuid(user_id)?;
        let rows = sqlx::query(
            "SELECT provider::text AS provider, profile_email, linked_at, last_authenticated_at \
             FROM provider_identities WHERE user_id = $1 AND revoked_at IS NULL \
             ORDER BY linked_at",
        )
        .bind(uuid)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| IdentityRow {
                provider: row.get("provider"),
                profile_email: row.get("profile_email"),
                linked_at: row.get("linked_at"),
                last_authenticated_at: row.get("last_authenticated_at"),
            })
            .collect())
    }

    /// List pending access requests for owners.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn list_access_requests(&self) -> AuthResult<Vec<AccessRequestRow>> {
        let rows = sqlx::query(
            "SELECT u.id::text AS user_id, pi.provider::text AS provider, \
             u.display_name, u.email, pi.linked_at AS requested_at \
             FROM users u \
             JOIN LATERAL ( \
               SELECT provider, linked_at FROM provider_identities \
               WHERE user_id = u.id ORDER BY linked_at LIMIT 1 \
             ) pi ON true \
             WHERE u.status = 'PENDING' ORDER BY u.created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| AccessRequestRow {
                user_id: row.get("user_id"),
                provider: row.get("provider"),
                display_name: row.get("display_name"),
                email: row.get("email"),
                requested_at: row.get("requested_at"),
            })
            .collect())
    }

    /// Approve a pending user as Viewer.
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] if the user is not pending.
    pub async fn approve_viewer(&self, user_id: &str) -> AuthResult<UserProjection> {
        let uuid = parse_uuid(user_id)?;
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE users SET status = 'ACTIVE', approved_at = now(), \
             permission_version = permission_version + 1 \
             WHERE id = $1 AND status = 'PENDING'",
        )
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(AuthError::NotFound);
        }
        sqlx::query(
            "INSERT INTO user_roles (user_id, role, active, activated_at) \
             VALUES ($1, 'VIEWER', true, now()) \
             ON CONFLICT (user_id, role) DO UPDATE SET active = true",
        )
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
        let user = load_user_projection(&mut tx, user_id).await?;
        tx.commit().await?;
        Ok(user)
    }

    /// Erase a user's PII and revoke their credentials.
    ///
    /// # Errors
    /// Returns [`AuthError::Conflict`] when deleting the final owner.
    pub async fn delete_account(&self, user_id: &str) -> AuthResult<()> {
        let uuid = parse_uuid(user_id)?;
        let mut tx = self.pool.begin().await?;
        let is_owner: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM user_roles WHERE user_id = $1 AND role = 'OWNER' AND active)",
        )
        .bind(uuid)
        .fetch_one(&mut *tx)
        .await?;
        if is_owner {
            let active_owners: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM user_roles ur JOIN users u ON u.id = ur.user_id \
                 WHERE ur.role = 'OWNER' AND ur.active AND u.status = 'ACTIVE'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if active_owners <= 1 {
                return Err(AuthError::Conflict(
                    "Transfer ownership before deleting the final Owner.",
                ));
            }
        }
        sqlx::query(
            "UPDATE users SET status = 'DELETED', email = NULL, display_name = NULL, \
             deletion_pseudonym = gen_random_uuid(), deleted_at = now(), \
             permission_version = permission_version + 1 WHERE id = $1",
        )
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE sessions SET state = 'REVOKED', revoked_at = now(), \
             revoke_reason = 'ACCOUNT_DELETED' WHERE user_id = $1 AND state = 'ACTIVE'",
        )
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE provider_identities SET revoked_at = now() \
             WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM user_roles WHERE user_id = $1")
            .bind(uuid)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // --- MFA ---

    /// Replace any pending TOTP method with a fresh unconfirmed one.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn begin_mfa(&self, user_id: &str, encrypted_secret: &[u8]) -> AuthResult<String> {
        let uuid = parse_uuid(user_id)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM mfa_methods WHERE user_id = $1 AND confirmed_at IS NULL")
            .bind(uuid)
            .execute(&mut *tx)
            .await?;
        let method_id: String = sqlx::query_scalar(
            "INSERT INTO mfa_methods (id, user_id, encrypted_totp_secret, key_version) \
             VALUES ($1, $2, $3, 1) RETURNING id::text",
        )
        .bind(Uuid::new_v4())
        .bind(uuid)
        .bind(encrypted_secret)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(method_id)
    }

    /// Load the newest unconfirmed TOTP method for a user.
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] if none is pending.
    pub async fn pending_mfa(&self, user_id: &str) -> AuthResult<MfaMethodRow> {
        let uuid = parse_uuid(user_id)?;
        let row = sqlx::query(
            "SELECT id::text AS id, encrypted_totp_secret, last_accepted_timestep \
             FROM mfa_methods \
             WHERE user_id = $1 AND confirmed_at IS NULL AND disabled_at IS NULL \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AuthError::NotFound)?;
        Ok(MfaMethodRow {
            id: row.get("id"),
            encrypted_secret: row.get("encrypted_totp_secret"),
            last_accepted_timestep: row.get("last_accepted_timestep"),
        })
    }

    /// Load the confirmed, active TOTP method for a user.
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] if none is confirmed.
    pub async fn confirmed_mfa(&self, user_id: &str) -> AuthResult<MfaMethodRow> {
        let uuid = parse_uuid(user_id)?;
        let row = sqlx::query(
            "SELECT id::text AS id, encrypted_totp_secret, last_accepted_timestep \
             FROM mfa_methods \
             WHERE user_id = $1 AND confirmed_at IS NOT NULL AND disabled_at IS NULL \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(AuthError::NotFound)?;
        Ok(MfaMethodRow {
            id: row.get("id"),
            encrypted_secret: row.get("encrypted_totp_secret"),
            last_accepted_timestep: row.get("last_accepted_timestep"),
        })
    }

    /// Confirm a TOTP method and store its recovery codes and digest.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn confirm_mfa(
        &self,
        method_id: &str,
        timestep: i64,
        recovery_hashes: &[[u8; 32]],
        digest: &[u8],
    ) -> AuthResult<()> {
        let uuid = parse_uuid(method_id)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE mfa_methods SET confirmed_at = now(), last_accepted_timestep = $2, \
             recovery_code_set_digest = $3 WHERE id = $1",
        )
        .bind(uuid)
        .bind(timestep)
        .bind(digest)
        .execute(&mut *tx)
        .await?;
        for hash in recovery_hashes {
            sqlx::query(
                "INSERT INTO recovery_codes (id, mfa_method_id, code_hash) VALUES ($1, $2, $3)",
            )
            .bind(Uuid::new_v4())
            .bind(uuid)
            .bind(&hash[..])
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Record the current TOTP timestep to prevent code reuse.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn record_totp_timestep(&self, method_id: &str, timestep: i64) -> AuthResult<()> {
        let uuid = parse_uuid(method_id)?;
        sqlx::query("UPDATE mfa_methods SET last_accepted_timestep = $2 WHERE id = $1")
            .bind(uuid)
            .bind(timestep)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Consume a recovery code if it matches a stored, unused hash.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn consume_recovery_code(&self, user_id: &str, code_hash: &[u8]) -> AuthResult<bool> {
        let uuid = parse_uuid(user_id)?;
        let result = sqlx::query(
            "UPDATE recovery_codes SET consumed_at = now() \
             WHERE code_hash = $1 AND consumed_at IS NULL AND mfa_method_id IN ( \
               SELECT id FROM mfa_methods WHERE user_id = $2 AND disabled_at IS NULL \
             )",
        )
        .bind(code_hash)
        .bind(uuid)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Fetch the stored recovery-code digest for a confirmed method.
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] if there is no confirmed method.
    pub async fn recovery_digest(&self, method_id: &str) -> AuthResult<Option<Vec<u8>>> {
        let uuid = parse_uuid(method_id)?;
        Ok(
            sqlx::query_scalar("SELECT recovery_code_set_digest FROM mfa_methods WHERE id = $1")
                .bind(uuid)
                .fetch_optional(&self.pool)
                .await?
                .flatten(),
        )
    }

    /// Record recovery-code acknowledgement.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn acknowledge_recovery(&self, method_id: &str, digest: &[u8]) -> AuthResult<()> {
        let uuid = parse_uuid(method_id)?;
        sqlx::query(
            "INSERT INTO recovery_code_acknowledgements \
             (mfa_method_id, acknowledged_at, code_set_digest) VALUES ($1, now(), $2) \
             ON CONFLICT (mfa_method_id) DO UPDATE \
             SET acknowledged_at = now(), code_set_digest = EXCLUDED.code_set_digest",
        )
        .bind(uuid)
        .bind(digest)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether the user has acknowledged their recovery codes.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn recovery_acknowledged(&self, user_id: &str) -> AuthResult<bool> {
        let uuid = parse_uuid(user_id)?;
        Ok(sqlx::query_scalar(
            "SELECT EXISTS( \
               SELECT 1 FROM recovery_code_acknowledgements a \
               JOIN mfa_methods m ON m.id = a.mfa_method_id \
               WHERE m.user_id = $1 AND m.disabled_at IS NULL)",
        )
        .bind(uuid)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Seed the singleton bootstrap row if absent.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn seed_bootstrap(&self, token_hash: &[u8], expires_at: &str) -> AuthResult<()> {
        let expiry: DateTime<Utc> = expires_at
            .parse()
            .map_err(|_| AuthError::Internal("bootstrap expiry is not RFC 3339".to_owned()))?;
        sqlx::query(
            "INSERT INTO bootstrap_state (singleton, deployment_token_hash, expires_at) \
             VALUES (true, $1, $2) ON CONFLICT (singleton) DO NOTHING",
        )
        .bind(token_hash)
        .bind(expiry)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Ensure a pseudonymized audit actor exists for a user, returning its
    /// `(actor_id, pseudonym)`. The pseudonym is stable across calls.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn ensure_audit_actor(&self, user_id: &str) -> AuthResult<(Uuid, Uuid)> {
        let row = sqlx::query(
            "INSERT INTO audit_actors (id, user_id, pseudonym) VALUES ($1, $2, $3)
             ON CONFLICT (user_id) DO UPDATE SET erased_at = audit_actors.erased_at
             RETURNING id, pseudonym",
        )
        .bind(Uuid::new_v4())
        .bind(parse_uuid(user_id)?)
        .bind(Uuid::new_v4())
        .fetch_one(&self.pool)
        .await?;
        Ok((row.get("id"), row.get("pseudonym")))
    }

    /// Append a hash-chained audit event for a tenant. Serializes per-tenant with
    /// a transaction-scoped advisory lock so the chain tail cannot fork.
    ///
    /// # Errors
    /// Propagates database failures.
    #[allow(clippy::too_many_arguments)]
    pub async fn append_audit(
        &self,
        tenant: TenantContext,
        actor_id: Uuid,
        actor_pseudonym: Uuid,
        event_type: &str,
        subject_type: &str,
        subject_id: &str,
        session_id: Option<Uuid>,
        details_json: &str,
    ) -> AuthResult<()> {
        let mut transaction = TenantTransaction::begin(&self.pool, tenant)
            .await
            .map_err(|error| AuthError::Internal(format!("tenant transaction: {error}")))?;
        // Serialize appends for this tenant so no two rows share a predecessor.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
            .bind(tenant.tenant_id().as_uuid())
            .execute(transaction.connection())
            .await?;
        let previous: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT event_hash FROM audit_events WHERE tenant_id = $1
             ORDER BY occurred_at DESC, id DESC LIMIT 1",
        )
        .bind(tenant.tenant_id().as_uuid())
        .fetch_optional(transaction.connection())
        .await?;
        let previous_array: Option<[u8; 32]> =
            previous.as_deref().and_then(|bytes| bytes.try_into().ok());

        let request_id = Uuid::new_v4();
        // Truncate to microseconds so the value hashed here equals the timestamptz
        // read back during verification.
        let occurred_at = Utc::now().trunc_subsecs(6);
        let canonical_form = canonical(
            event_type,
            subject_type,
            subject_id,
            &actor_pseudonym.to_string(),
            &request_id.to_string(),
            &session_id.map(|id| id.to_string()).unwrap_or_default(),
            details_json,
            &format_audit_ts(occurred_at),
        );
        let hash = event_hash(previous_array.as_ref(), &canonical_form);

        sqlx::query(
            "INSERT INTO audit_events
                (id, tenant_id, actor_id, event_type, subject_type, subject_id,
                 session_id, request_id, details, previous_hash, event_hash, occurred_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb, $10, $11, $12)",
        )
        .bind(Uuid::new_v4())
        .bind(tenant.tenant_id().as_uuid())
        .bind(actor_id)
        .bind(event_type)
        .bind(subject_type)
        .bind(subject_id)
        .bind(session_id)
        .bind(request_id)
        .bind(details_json)
        .bind(previous.as_deref())
        .bind(hash.as_slice())
        .bind(occurred_at)
        .execute(transaction.connection())
        .await?;
        transaction
            .commit()
            .await
            .map_err(|error| AuthError::Internal(format!("commit audit: {error}")))?;
        Ok(())
    }

    /// Walk a tenant's audit chain and confirm every link. Returns `true` when the
    /// chain is intact, `false` when any row's hash or linkage has been altered.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn verify_audit_chain(&self, tenant: TenantContext) -> AuthResult<bool> {
        let mut transaction = TenantTransaction::begin(&self.pool, tenant)
            .await
            .map_err(|error| AuthError::Internal(format!("tenant transaction: {error}")))?;
        let rows = sqlx::query(
            "SELECT e.event_type, e.subject_type, e.subject_id, e.request_id, e.session_id,
                    e.details::text AS details, e.previous_hash, e.event_hash, e.occurred_at,
                    a.pseudonym AS pseudonym
             FROM audit_events e JOIN audit_actors a ON a.id = e.actor_id
             WHERE e.tenant_id = $1 ORDER BY e.occurred_at ASC, e.id ASC",
        )
        .bind(tenant.tenant_id().as_uuid())
        .fetch_all(transaction.connection())
        .await?;
        transaction
            .commit()
            .await
            .map_err(|error| AuthError::Internal(format!("commit verify: {error}")))?;

        let mut previous: Option<[u8; 32]> = None;
        for row in rows {
            let stored_previous: Option<Vec<u8>> = row.get("previous_hash");
            // The stored back-link must match the running predecessor hash.
            if stored_previous.as_deref() != previous.as_ref().map(|hash| hash.as_slice()) {
                return Ok(false);
            }
            let details: String = row.get("details");
            let normalized = serde_json::from_str::<serde_json::Value>(&details)
                .map(|value| value.to_string())
                .unwrap_or(details);
            let session: Option<Uuid> = row.get("session_id");
            let request_id: Uuid = row.get("request_id");
            let pseudonym: Uuid = row.get("pseudonym");
            let occurred_at: DateTime<Utc> = row.get("occurred_at");
            let canonical_form = canonical(
                &row.get::<String, _>("event_type"),
                &row.get::<String, _>("subject_type"),
                &row.get::<String, _>("subject_id"),
                &pseudonym.to_string(),
                &request_id.to_string(),
                &session.map(|id| id.to_string()).unwrap_or_default(),
                &normalized,
                &format_audit_ts(occurred_at),
            );
            let computed = event_hash(previous.as_ref(), &canonical_form);
            let stored_hash: Vec<u8> = row.get("event_hash");
            if computed.as_slice() != stored_hash.as_slice() {
                return Ok(false);
            }
            previous = Some(computed);
        }
        Ok(true)
    }

    /// List a tenant's audit events, newest first, with the pseudonymized actor.
    /// Exposes only non-secret display fields — never `previous_hash`, session, or
    /// request identifiers.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn list_audit(&self, tenant: TenantContext) -> AuthResult<Vec<AuditRow>> {
        let mut transaction = TenantTransaction::begin(&self.pool, tenant)
            .await
            .map_err(|error| AuthError::Internal(format!("tenant transaction: {error}")))?;
        let rows = sqlx::query(
            "SELECT e.occurred_at, e.event_type, e.subject_type, e.subject_id,
                    e.details::text AS details, e.event_hash, a.pseudonym AS pseudonym
             FROM audit_events e JOIN audit_actors a ON a.id = e.actor_id
             WHERE e.tenant_id = $1 ORDER BY e.occurred_at DESC, e.id DESC",
        )
        .bind(tenant.tenant_id().as_uuid())
        .fetch_all(transaction.connection())
        .await?;
        transaction
            .commit()
            .await
            .map_err(|error| AuthError::Internal(format!("commit list audit: {error}")))?;
        Ok(rows
            .into_iter()
            .map(|row| AuditRow {
                occurred_at: row.get("occurred_at"),
                event_type: row.get("event_type"),
                subject_type: row.get("subject_type"),
                subject_id: row.get("subject_id"),
                details: row.get("details"),
                event_hash: row.get("event_hash"),
                pseudonym: row.get("pseudonym"),
            })
            .collect())
    }
}

/// A session row used by the refresh path.
#[derive(Clone, Debug)]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub tenant_id: String,
    pub device_id: String,
    pub state: String,
    pub generation: i64,
    pub permission_version: i64,
    pub expires_at: DateTime<Utc>,
}

/// Bootstrap singleton row.
#[derive(Clone, Debug)]
pub struct BootstrapRow {
    pub deployment_token_hash: Vec<u8>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

/// Session summary row for listing.
#[derive(Clone, Debug)]
pub struct SessionSummaryRow {
    pub id: String,
    pub device_id: String,
    pub device_name: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// Identity row for listing.
#[derive(Clone, Debug)]
pub struct IdentityRow {
    pub provider: String,
    pub profile_email: Option<String>,
    pub linked_at: DateTime<Utc>,
    pub last_authenticated_at: DateTime<Utc>,
}

/// Access-request row for owners.
#[derive(Clone, Debug)]
pub struct AccessRequestRow {
    pub user_id: String,
    pub provider: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub requested_at: DateTime<Utc>,
}

/// An audit-event row for owner review (non-secret display fields only).
#[derive(Clone, Debug)]
pub struct AuditRow {
    pub occurred_at: DateTime<Utc>,
    pub event_type: String,
    pub subject_type: String,
    pub subject_id: String,
    pub details: String,
    pub event_hash: Vec<u8>,
    pub pseudonym: Uuid,
}

/// A TOTP method row.
#[derive(Clone, Debug)]
pub struct MfaMethodRow {
    pub id: String,
    pub encrypted_secret: Vec<u8>,
    pub last_accepted_timestep: Option<i64>,
}

/// Format an epoch-aware timestamp for the wire.
#[must_use]
pub fn to_rfc3339(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}

/// Fixed-precision (microsecond) UTC timestamp used in the audit hash so a value
/// hashed on append equals the same row read back during verification.
fn format_audit_ts(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string()
}

fn parse_uuid(value: &str) -> AuthResult<Uuid> {
    Uuid::parse_str(value).map_err(|_| AuthError::BadRequest("identifier is not a valid UUID"))
}

/// Insert a fresh user (and, under open registration, the full privileged role
/// set), returning the new id. Does not touch `provider_identities`.
async fn insert_new_user(
    conn: &mut sqlx::PgConnection,
    identity: &IdentityUpsert,
    auto_approve: bool,
) -> AuthResult<Uuid> {
    let new_user = Uuid::new_v4();
    let new_tenant = Uuid::new_v4();
    // Open registration (DEC-0009) admits new users to full access immediately;
    // otherwise they start PENDING an Owner's approval.
    let status = if auto_approve { "ACTIVE" } else { "PENDING" };
    sqlx::query(
        "INSERT INTO users (id, status, email, display_name, approved_at) \
         VALUES ($1, $2::user_status, $3, $4, CASE WHEN $5 THEN now() END)",
    )
    .bind(new_user)
    .bind(status)
    .bind(&identity.email)
    .bind(&identity.display_name)
    .bind(auto_approve)
    .execute(&mut *conn)
    .await?;
    let tenant_name = identity
        .display_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Personal portfolio");
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, LEFT($2, 120))")
        .bind(new_tenant)
        .bind(tenant_name)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO tenant_memberships (tenant_id, user_id, role, active) \
         VALUES ($1, $2, 'OWNER', true)",
    )
    .bind(new_tenant)
    .bind(new_user)
    .execute(&mut *conn)
    .await?;
    if auto_approve {
        for role in ["OWNER", "TRADER", "APPROVER"] {
            sqlx::query(
                "INSERT INTO user_roles (user_id, role, active, activated_at) \
                 VALUES ($1, $2::user_role, true, now())",
            )
            .bind(new_user)
            .bind(role)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(new_user)
}

async fn insert_session<'e, E>(
    executor: E,
    session_id: &str,
    user_id: &str,
    device_id: &str,
    refresh_hash: &[u8],
    permission_version: i64,
) -> AuthResult<()>
where
    E: sqlx::PgExecutor<'e>,
{
    let session_uuid = parse_uuid(session_id)?;
    let user_uuid = parse_uuid(user_id)?;
    let device_uuid = parse_uuid(device_id)?;
    let expires_at = Utc::now() + Duration::days(SESSION_TTL_DAYS);
    let result = sqlx::query(
        "INSERT INTO sessions \
         (id, user_id, tenant_id, device_id, family_id, refresh_token_hash, generation, \
          permission_version, expires_at) \
         SELECT $1, $2, tm.tenant_id, $3, $4, $5, 0, $6, $7 \
         FROM tenant_memberships tm WHERE tm.user_id = $2 AND tm.active",
    )
    .bind(session_uuid)
    .bind(user_uuid)
    .bind(device_uuid)
    .bind(Uuid::new_v4())
    .bind(refresh_hash)
    .bind(permission_version)
    .bind(expires_at)
    .execute(executor)
    .await?;
    if result.rows_affected() != 1 {
        return Err(AuthError::Forbidden);
    }
    Ok(())
}

async fn load_user_projection(
    conn: &mut sqlx::PgConnection,
    user_id: &str,
) -> AuthResult<UserProjection> {
    let uuid = parse_uuid(user_id)?;
    let base = sqlx::query(
        "SELECT u.status::text AS status, u.email, u.display_name, u.permission_version, \
                t.id::text AS tenant_id, t.display_name AS tenant_display_name, \
                t.base_currency, t.state::text AS tenant_state \
         FROM users u \
         JOIN tenant_memberships tm ON tm.user_id = u.id AND tm.active \
         JOIN tenants t ON t.id = tm.tenant_id AND t.state <> 'DELETED' \
         WHERE u.id = $1",
    )
    .bind(uuid)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AuthError::NotFound)?;

    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT role::text FROM user_roles WHERE user_id = $1 AND active = true",
    )
    .bind(uuid)
    .fetch_all(&mut *conn)
    .await?;

    let providers: Vec<String> = sqlx::query_scalar(
        "SELECT provider::text FROM provider_identities \
         WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(uuid)
    .fetch_all(&mut *conn)
    .await?;

    let mfa_enrolled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mfa_methods \
         WHERE user_id = $1 AND confirmed_at IS NOT NULL AND disabled_at IS NULL)",
    )
    .bind(uuid)
    .fetch_one(&mut *conn)
    .await?;

    Ok(UserProjection {
        id: user_id.to_owned(),
        tenant_id: base.get("tenant_id"),
        tenant_display_name: base.get("tenant_display_name"),
        tenant_base_currency: base.get("base_currency"),
        tenant_state: base.get("tenant_state"),
        status: base.get("status"),
        email: base.get("email"),
        display_name: base.get("display_name"),
        permission_version: base.get("permission_version"),
        roles,
        providers,
        mfa_enrolled,
    })
}
