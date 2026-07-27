//! Application services orchestrating verification, persistence, and tokens.
//!
//! Invariants (challenge single-use, refresh rotation, bootstrap single-use)
//! live in `funding_arb_domain::auth`; this layer wires adapters to them.

use std::sync::Arc;

use funding_arb_domain::pilot::PilotState;
use funding_arb_tenant_context::{TenantContext, TenantId};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::config::AuthConfig;
use crate::dto::{
    AccessRequest, AcknowledgeRecoveryRequest, AuditEntry, AuditLog, AuthTransition,
    BootstrapRequest, CreateChallengeRequest, CurrentUser, ExchangeRequest, LinkedIdentity,
    MfaConfirmRequest, MfaConfirmResponse, MfaEnrollment, MfaVerifyRequest, NextState, Provider,
    RefreshRequest, Role, SessionCredentials, SessionSummary, SocialChallenge, TenantSummary,
    UserStatus,
};
use crate::error::{AuthError, AuthResult};
use crate::mfa;
use crate::providers::IdentityTokenVerifier;
use crate::repo::{DeviceUpsert, IdentityUpsert, Repo, UserProjection, to_rfc3339};
use crate::tokens::{
    TokenSigner, decode_base64url, encode_base64url, generate_recovery_codes,
    generate_refresh_token, sha256,
};

const CHALLENGE_TTL_SECONDS: i64 = 300;
const RECOVERY_CODE_COUNT: usize = 8;
const OTP_ISSUER: &str = "Perpeto";

/// Authenticated caller context resolved by the transport middleware.
#[derive(Clone, Debug)]
pub struct CallerContext {
    pub user_id: String,
    pub tenant_id: String,
    pub session_id: String,
    pub device_id: String,
    pub status: String,
    pub roles: Vec<String>,
}

impl CallerContext {
    /// Immutable tenant context derived from the verified session claim.
    pub fn tenant_context(&self) -> AuthResult<TenantContext> {
        self.tenant_id
            .parse::<TenantId>()
            .map(TenantContext::new)
            .map_err(|_| AuthError::Unauthorized)
    }

    /// Whether the caller holds an active owner role.
    #[must_use]
    pub fn is_owner(&self) -> bool {
        self.roles.iter().any(|role| role == "OWNER")
    }

    /// Whether the caller may open/manage trading positions (owner or trader).
    #[must_use]
    pub fn can_trade(&self) -> bool {
        self.roles
            .iter()
            .any(|role| role == "OWNER" || role == "TRADER")
    }

    /// Require the account to be fully active.
    ///
    /// # Errors
    /// Returns [`AuthError::Forbidden`] for non-active accounts.
    pub fn require_active(&self) -> AuthResult<()> {
        if self.status == "ACTIVE" {
            Ok(())
        } else {
            Err(AuthError::Forbidden)
        }
    }
}

/// The auth application service.
#[derive(Clone)]
pub struct AuthService {
    repo: Repo,
    signer: TokenSigner,
    verifier: Arc<dyn IdentityTokenVerifier>,
    mfa_key: Option<[u8; 32]>,
    open_registration: bool,
}

impl AuthService {
    /// Construct the service from its adapters.
    #[must_use]
    pub fn new(repo: Repo, config: &AuthConfig, verifier: Arc<dyn IdentityTokenVerifier>) -> Self {
        Self {
            repo,
            signer: TokenSigner::new(config.token_signing_seed, config.token_issuer.clone()),
            verifier,
            mfa_key: config.mfa_encryption_key,
            open_registration: config.open_registration,
        }
    }

    /// Access the repository (used by transport for readiness checks).
    #[must_use]
    pub const fn repo(&self) -> &Repo {
        &self.repo
    }

    /// The caller's tenant pilot state.
    ///
    /// # Errors
    /// Returns [`AuthError`] when the tenant is missing or its state is unknown.
    pub async fn pilot_state(&self, caller: &CallerContext) -> AuthResult<PilotState> {
        let wire = self.repo.pilot_state(&caller.tenant_id).await?;
        PilotState::from_wire(&wire)
            .ok_or_else(|| AuthError::Internal(format!("unknown tenant state: {wire}")))
    }

    /// Persist a pilot-state transition for the caller's own tenant.
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] when the tenant is not live.
    pub async fn set_pilot_state(
        &self,
        caller: &CallerContext,
        state: PilotState,
    ) -> AuthResult<()> {
        self.repo.set_pilot_state(&caller.tenant_id, state.wire()).await
    }

    /// Append a hash-chained audit event for a sensitive control-plane action,
    /// resolving (and creating if needed) the caller's pseudonymized actor.
    ///
    /// # Errors
    /// Returns [`AuthError`] when the actor cannot be resolved or the append fails.
    pub async fn record_audit(
        &self,
        caller: &CallerContext,
        event_type: &str,
        subject_type: &str,
        subject_id: &str,
        details: &serde_json::Value,
    ) -> AuthResult<()> {
        let tenant = caller.tenant_context()?;
        let (actor_id, pseudonym) = self.repo.ensure_audit_actor(&caller.user_id).await?;
        let session_id = caller.session_id.parse::<Uuid>().ok();
        self.repo
            .append_audit(
                tenant,
                actor_id,
                pseudonym,
                event_type,
                subject_type,
                subject_id,
                session_id,
                &details.to_string(),
            )
            .await
    }

    /// Verify an access token minted by this service.
    ///
    /// # Errors
    /// Returns [`AuthError::Unauthorized`] for invalid tokens.
    pub fn verify_access_token(&self, token: &str) -> AuthResult<crate::tokens::AccessClaims> {
        self.signer.verify_access_token(token)
    }

    /// Load a user's public projection (for middleware).
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] if the user is gone.
    pub async fn user_projection(&self, user_id: &str) -> AuthResult<UserProjection> {
        self.repo.user_projection(user_id).await
    }

    /// Create a one-use social sign-in challenge.
    ///
    /// # Errors
    /// Returns [`AuthError::BadRequest`] for malformed input.
    pub async fn create_challenge(
        &self,
        request: CreateChallengeRequest,
    ) -> AuthResult<SocialChallenge> {
        if request.purpose != "SIGN_IN" {
            return Err(AuthError::BadRequest(
                "only SIGN_IN challenges are available",
            ));
        }
        let challenge_len = request.code_challenge.len();
        if !(43..=128).contains(&challenge_len) {
            return Err(AuthError::BadRequest(
                "code_challenge has an invalid length",
            ));
        }
        let state = encode_base64url(&random_bytes());
        let nonce = encode_base64url(&random_bytes());
        let state_hash = sha256(state.as_bytes());
        let nonce_hash = sha256(nonce.as_bytes());
        let id = self
            .repo
            .insert_challenge(
                request.provider.as_db(),
                "SIGN_IN",
                None,
                &state_hash,
                &nonce_hash,
                &request.code_challenge,
                &request.redirect_uri,
                CHALLENGE_TTL_SECONDS,
            )
            .await?;
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(CHALLENGE_TTL_SECONDS);
        Ok(SocialChallenge {
            id,
            provider: request.provider,
            state,
            nonce,
            expires_at: to_rfc3339(expires_at),
        })
    }

    /// Verify a provider proof and establish (or advance) the account.
    ///
    /// # Errors
    /// Returns provider, PKCE, or challenge errors as appropriate.
    pub async fn exchange(&self, request: ExchangeRequest) -> AuthResult<AuthTransition> {
        let challenge = self
            .repo
            .load_challenge(&request.challenge_id)
            .await?
            .ok_or(AuthError::ChallengeConsumed)?;
        if challenge.purpose != "SIGN_IN" {
            return Err(AuthError::BadRequest("challenge purpose mismatch"));
        }
        if challenge.consumed_at.is_some() || challenge.expires_at <= chrono::Utc::now() {
            return Err(AuthError::ChallengeConsumed);
        }
        let provider = Provider::from_db(&challenge.provider)
            .ok_or_else(|| AuthError::Internal("unknown provider on challenge".to_owned()))?;

        let verifier = request
            .code_verifier
            .as_deref()
            .ok_or(AuthError::BadRequest("code_verifier is required"))?;
        let recomputed = encode_base64url(&sha256(verifier.as_bytes()));
        if recomputed
            .as_bytes()
            .ct_eq(challenge.pkce_challenge.as_bytes())
            .unwrap_u8()
            != 1
        {
            return Err(AuthError::ProviderProofInvalid);
        }

        let identity = self
            .verifier
            .verify(provider, &request.identity_token)
            .await?;

        if provider == Provider::Apple {
            let nonce = identity
                .nonce
                .as_deref()
                .ok_or(AuthError::ProviderProofInvalid)?;
            if sha256(nonce.as_bytes())
                .ct_eq(challenge.nonce_hash.as_slice())
                .unwrap_u8()
                != 1
            {
                return Err(AuthError::ProviderProofInvalid);
            }
        }

        if request.device.platform != "IOS" {
            return Err(AuthError::BadRequest("unsupported device platform"));
        }
        let display_name = match provider {
            Provider::Apple => request
                .apple_profile
                .as_ref()
                .and_then(|profile| profile.display_name.clone())
                .or(identity.display_name.clone()),
            Provider::Google => identity.display_name.clone(),
        };
        let is_private_relay = identity
            .email
            .as_deref()
            .is_some_and(|email| email.ends_with("privaterelay.appleid.com"));

        let identity_upsert = IdentityUpsert {
            provider: provider.as_db().to_owned(),
            subject: identity.subject.clone(),
            email: identity.email.clone(),
            is_private_relay,
            display_name,
        };
        let device_upsert = DeviceUpsert {
            installation_id_hash: sha256(request.device.installation_id.as_bytes()).to_vec(),
            app_version: request.device.app_version.clone(),
            display_name: request.device.display_name.clone(),
        };

        let outcome = self
            .repo
            .commit_exchange(
                &request.challenge_id,
                &identity_upsert,
                &device_upsert,
                None,
                None,
                self.open_registration,
            )
            .await?;
        let next = self.compute_next(&outcome.user).await?;

        let mut transition = AuthTransition {
            next,
            user: to_current_user(&outcome.user),
            challenge_token: None,
            session: None,
        };

        let issue_session = matches!(
            next,
            NextState::PendingApproval
                | NextState::Authenticated
                | NextState::MfaEnrollmentRequired
        );
        if issue_session {
            transition.session = Some(
                self.open_session(
                    &outcome.user.id,
                    &outcome.device_id,
                    outcome.user.permission_version,
                )
                .await?,
            );
        }
        if matches!(next, NextState::MfaRequired) {
            transition.challenge_token = Some(self.signer.mint_challenge_token(
                &outcome.user.id,
                &outcome.device_id,
                "mfa",
            )?);
        }
        if matches!(next, NextState::MfaEnrollmentRequired) {
            transition.challenge_token = Some(self.signer.mint_challenge_token(
                &outcome.user.id,
                &outcome.device_id,
                "mfa_enroll",
            )?);
        }
        Ok(transition)
    }

    /// Rotate a refresh token, detecting replay.
    ///
    /// # Errors
    /// Returns refresh-specific errors that invalidate the client session.
    pub async fn refresh(&self, request: RefreshRequest) -> AuthResult<SessionCredentials> {
        let presented_hash = sha256(request.refresh_token.as_bytes());
        let Some(session) = self.repo.session_by_refresh_hash(&presented_hash).await? else {
            if let Some(session_id) = self
                .repo
                .session_for_historic_token(&presented_hash)
                .await?
            {
                self.repo.compromise_session(&session_id).await?;
                return Err(AuthError::RefreshTokenReuse);
            }
            return Err(AuthError::InvalidRefreshToken);
        };

        if session.state != "ACTIVE"
            || session.device_id != request.device_id
            || session.expires_at <= chrono::Utc::now()
        {
            return Err(AuthError::InvalidRefreshToken);
        }

        let user = self.repo.user_projection(&session.user_id).await?;
        if user.tenant_id != session.tenant_id {
            self.repo
                .revoke_session_internal(&session.id, "TENANT_MEMBERSHIP_CHANGED")
                .await?;
            return Err(AuthError::SessionRevoked);
        }
        match user.status.as_str() {
            "DELETED" => return Err(AuthError::AccountDeleted),
            "SUSPENDED" => return Err(AuthError::UserSuspended),
            _ => {}
        }
        if user.permission_version != session.permission_version {
            self.repo
                .revoke_session_internal(&session.id, "PERMISSION_ROTATED")
                .await?;
            return Err(AuthError::SessionRevoked);
        }

        let new_token = generate_refresh_token();
        let new_hash = sha256(new_token.as_bytes());
        self.repo
            .rotate_refresh(&session.id, &presented_hash, session.generation, &new_hash)
            .await?;
        let (access_token, expires_at) = self.signer.mint_access_token(
            &session.user_id,
            &session.tenant_id,
            &session.id,
            &session.device_id,
            session.permission_version,
        )?;
        Ok(SessionCredentials {
            access_token,
            access_token_expires_at: to_rfc3339(from_epoch(expires_at)),
            refresh_token: new_token,
            session_id: session.id,
            device_id: session.device_id,
        })
    }

    /// Return the caller's public projection.
    ///
    /// # Errors
    /// Propagates lookup failures.
    pub async fn me(&self, user_id: &str) -> AuthResult<CurrentUser> {
        Ok(to_current_user(&self.repo.user_projection(user_id).await?))
    }

    /// Consume the deployment bootstrap token to seat the first owner.
    ///
    /// # Errors
    /// Returns [`AuthError::BootstrapInvalid`] on any token failure.
    pub async fn bootstrap(
        &self,
        caller: &CallerContext,
        request: BootstrapRequest,
    ) -> AuthResult<AuthTransition> {
        let state = self
            .repo
            .bootstrap_state()
            .await?
            .ok_or(AuthError::BootstrapInvalid)?;
        if state.consumed_at.is_some() || state.expires_at <= chrono::Utc::now() {
            return Err(AuthError::BootstrapInvalid);
        }
        let presented = sha256(request.deployment_token.as_bytes());
        if presented
            .ct_eq(state.deployment_token_hash.as_slice())
            .unwrap_u8()
            != 1
        {
            return Err(AuthError::BootstrapInvalid);
        }

        let user = self
            .repo
            .commit_bootstrap(&caller.user_id, &caller.session_id)
            .await?;
        let next = self.compute_next(&user).await?;
        let mut transition = AuthTransition {
            next,
            user: to_current_user(&user),
            challenge_token: None,
            session: None,
        };
        match next {
            NextState::MfaEnrollmentRequired => {
                transition.challenge_token = Some(self.signer.mint_challenge_token(
                    &user.id,
                    &caller.device_id,
                    "mfa_enroll",
                )?);
            }
            NextState::MfaRequired => {
                transition.challenge_token = Some(self.signer.mint_challenge_token(
                    &user.id,
                    &caller.device_id,
                    "mfa",
                )?);
            }
            NextState::Authenticated => {
                transition.session = Some(
                    self.open_session(&user.id, &caller.device_id, user.permission_version)
                        .await?,
                );
            }
            NextState::PendingApproval => {}
        }
        Ok(transition)
    }

    /// Revoke the caller's current session.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn logout(&self, caller: &CallerContext) -> AuthResult<()> {
        self.repo.logout(&caller.session_id).await
    }

    /// List the caller's active sessions.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn list_sessions(&self, caller: &CallerContext) -> AuthResult<Vec<SessionSummary>> {
        let rows = self.repo.list_sessions(&caller.user_id).await?;
        Ok(rows
            .into_iter()
            .map(|row| SessionSummary {
                current: row.id == caller.session_id,
                id: row.id,
                device_id: row.device_id,
                device_name: row.device_name,
                created_at: to_rfc3339(row.created_at),
                last_seen_at: to_rfc3339(row.last_seen_at),
            })
            .collect())
    }

    /// Revoke another of the caller's sessions.
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] for unknown sessions.
    pub async fn revoke_session(&self, caller: &CallerContext, session_id: &str) -> AuthResult<()> {
        self.repo
            .revoke_own_session(&caller.user_id, session_id)
            .await
    }

    /// List the caller's linked identities.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn list_identities(&self, caller: &CallerContext) -> AuthResult<Vec<LinkedIdentity>> {
        let rows = self.repo.list_identities(&caller.user_id).await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Provider::from_db(&row.provider).map(|provider| LinkedIdentity {
                    provider,
                    profile_email: row.profile_email,
                    linked_at: to_rfc3339(row.linked_at),
                    last_authenticated_at: to_rfc3339(row.last_authenticated_at),
                })
            })
            .collect())
    }

    /// List pending access requests (owners only).
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn list_access_requests(&self) -> AuthResult<Vec<AccessRequest>> {
        let rows = self.repo.list_access_requests().await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Provider::from_db(&row.provider).map(|provider| AccessRequest {
                    user_id: row.user_id,
                    provider,
                    display_name: row.display_name,
                    email: row.email,
                    requested_at: to_rfc3339(row.requested_at),
                })
            })
            .collect())
    }

    /// The caller tenant's audit log plus a tamper-evidence integrity flag.
    ///
    /// # Errors
    /// Returns [`AuthError`] when the tenant context is invalid or a query fails.
    pub async fn audit_log(&self, caller: &CallerContext) -> AuthResult<AuditLog> {
        let tenant = caller.tenant_context()?;
        let intact = self.repo.verify_audit_chain(tenant).await?;
        let events = self
            .repo
            .list_audit(tenant)
            .await?
            .into_iter()
            .map(|row| AuditEntry {
                occurred_at: to_rfc3339(row.occurred_at),
                event_type: row.event_type,
                subject_type: row.subject_type,
                subject_id: row.subject_id,
                actor: row.pseudonym.to_string(),
                details: serde_json::from_str(&row.details).unwrap_or(serde_json::Value::Null),
                event_hash: row.event_hash.iter().map(|byte| format!("{byte:02x}")).collect(),
            })
            .collect();
        Ok(AuditLog { intact, events })
    }

    /// Approve a pending user as Viewer (owners only).
    ///
    /// # Errors
    /// Returns [`AuthError::NotFound`] if the user is not pending.
    pub async fn approve_access_request(&self, user_id: &str) -> AuthResult<CurrentUser> {
        Ok(to_current_user(&self.repo.approve_viewer(user_id).await?))
    }

    /// Erase the caller's account.
    ///
    /// # Errors
    /// Returns [`AuthError::Conflict`] when deleting the final owner.
    pub async fn delete_account(&self, caller: &CallerContext) -> AuthResult<()> {
        self.repo.delete_account(&caller.user_id).await
    }

    // --- MFA ---

    /// Begin TOTP enrollment for the caller.
    ///
    /// # Errors
    /// Returns [`AuthError::Internal`] if MFA is not configured.
    pub async fn enroll_mfa(&self, caller: &CallerContext) -> AuthResult<MfaEnrollment> {
        let key = self.mfa_key()?;
        let secret = mfa::generate_secret();
        let encrypted = mfa::encrypt_secret(&key, &secret)?;
        let method_id = self.repo.begin_mfa(&caller.user_id, &encrypted).await?;
        let user = self.repo.user_projection(&caller.user_id).await?;
        let account = user.email.unwrap_or_else(|| caller.user_id.clone());
        Ok(MfaEnrollment {
            method_id,
            otpauth_uri: mfa::otpauth_uri(OTP_ISSUER, &account, &secret),
            secret,
        })
    }

    /// Confirm a TOTP method and issue recovery codes.
    ///
    /// # Errors
    /// Returns [`AuthError::BadRequest`] for rejected codes.
    pub async fn confirm_mfa(&self, request: MfaConfirmRequest) -> AuthResult<MfaConfirmResponse> {
        let key = self.mfa_key()?;
        let claims = self
            .signer
            .verify_challenge_token(&request.challenge_token, "mfa_enroll")?;
        let method = self.repo.pending_mfa(&claims.sub).await?;
        let secret = mfa::decrypt_secret(&key, &method.encrypted_secret)?;
        let step = mfa::verify_totp(
            &secret,
            &request.code,
            now_epoch(),
            method.last_accepted_timestep,
        )?
        .ok_or(AuthError::BadRequest("That code was not accepted."))?;

        let codes = generate_recovery_codes(RECOVERY_CODE_COUNT);
        let hashes: Vec<[u8; 32]> = codes.iter().map(|code| sha256(code.as_bytes())).collect();
        let serialized = serde_json::to_string(&codes)
            .map_err(|error| AuthError::Internal(format!("recovery serialize: {error}")))?;
        let digest = sha256(serialized.as_bytes());
        self.repo
            .confirm_mfa(&method.id, step, &hashes, &digest)
            .await?;
        Ok(MfaConfirmResponse {
            recovery_codes: codes,
        })
    }

    /// Verify a TOTP or recovery code and open a session.
    ///
    /// # Errors
    /// Returns [`AuthError::BadRequest`] for rejected codes.
    pub async fn verify_mfa(&self, request: MfaVerifyRequest) -> AuthResult<SessionCredentials> {
        let key = self.mfa_key()?;
        let claims = self
            .signer
            .verify_challenge_token(&request.challenge_token, "mfa")?;
        let method = self.repo.confirmed_mfa(&claims.sub).await?;

        if request.recovery_code {
            let hash = sha256(request.code.trim().as_bytes());
            if !self.repo.consume_recovery_code(&claims.sub, &hash).await? {
                return Err(AuthError::BadRequest("That code was not accepted."));
            }
        } else {
            let secret = mfa::decrypt_secret(&key, &method.encrypted_secret)?;
            let step = mfa::verify_totp(
                &secret,
                &request.code,
                now_epoch(),
                method.last_accepted_timestep,
            )?
            .ok_or(AuthError::BadRequest("That code was not accepted."))?;
            self.repo.record_totp_timestep(&method.id, step).await?;
        }

        let user = self.repo.user_projection(&claims.sub).await?;
        self.open_session(&claims.sub, &claims.did, user.permission_version)
            .await
    }

    /// Acknowledge recovery codes and open the first authenticated session.
    ///
    /// # Errors
    /// Returns [`AuthError::BadRequest`] on digest mismatch.
    pub async fn acknowledge_recovery(
        &self,
        request: AcknowledgeRecoveryRequest,
    ) -> AuthResult<SessionCredentials> {
        let claims = self
            .signer
            .verify_challenge_token(&request.challenge_token, "mfa_enroll")?;
        let method = self.repo.confirmed_mfa(&claims.sub).await?;
        let stored = self
            .repo
            .recovery_digest(&method.id)
            .await?
            .ok_or(AuthError::BadRequest("recovery codes were not issued"))?;
        let provided = decode_hex_32(&request.code_set_digest)
            .ok_or(AuthError::BadRequest("recovery digest is malformed"))?;
        if provided.ct_eq(stored.as_slice()).unwrap_u8() != 1 {
            return Err(AuthError::BadRequest("recovery digest does not match"));
        }
        self.repo.acknowledge_recovery(&method.id, &stored).await?;

        let user = self.repo.user_projection(&claims.sub).await?;
        self.open_session(&claims.sub, &claims.did, user.permission_version)
            .await
    }

    async fn open_session(
        &self,
        user_id: &str,
        device_id: &str,
        permission_version: i64,
    ) -> AuthResult<SessionCredentials> {
        let session_id = Uuid::new_v4().to_string();
        let refresh_token = generate_refresh_token();
        let refresh_hash = sha256(refresh_token.as_bytes());
        self.repo
            .create_session(
                &session_id,
                user_id,
                device_id,
                &refresh_hash,
                permission_version,
            )
            .await?;
        let user = self.repo.user_projection(user_id).await?;
        let (access_token, expires_at) = self.signer.mint_access_token(
            user_id,
            &user.tenant_id,
            &session_id,
            device_id,
            permission_version,
        )?;
        Ok(SessionCredentials {
            access_token,
            access_token_expires_at: to_rfc3339(from_epoch(expires_at)),
            refresh_token,
            session_id,
            device_id: device_id.to_owned(),
        })
    }

    async fn compute_next(&self, user: &UserProjection) -> AuthResult<NextState> {
        if user.status != "ACTIVE" {
            return Ok(NextState::PendingApproval);
        }
        // Open registration (DEC-0009) waives MFA for the shared v1 backend: an
        // active user goes straight in regardless of privileged roles.
        if self.open_registration {
            return Ok(NextState::Authenticated);
        }
        let privileged = user
            .roles
            .iter()
            .any(|role| matches!(role.as_str(), "OWNER" | "TRADER" | "APPROVER"));
        if privileged {
            let acknowledged = self.repo.recovery_acknowledged(&user.id).await?;
            if !user.mfa_enrolled || !acknowledged {
                return Ok(NextState::MfaEnrollmentRequired);
            }
            return Ok(NextState::MfaRequired);
        }
        Ok(NextState::Authenticated)
    }

    fn mfa_key(&self) -> AuthResult<[u8; 32]> {
        self.mfa_key
            .ok_or_else(|| AuthError::Internal("MFA is not configured".to_owned()))
    }
}

fn random_bytes() -> [u8; 32] {
    use rand::RngCore;
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

fn now_epoch() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn from_epoch(epoch_seconds: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(epoch_seconds, 0).unwrap_or_else(chrono::Utc::now)
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if let Some(bytes) = decode_base64url(value).ok()
        && let Ok(array) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return Some(array);
    }
    let trimmed = value.trim();
    if trimmed.len() != 64 {
        return None;
    }
    let mut out = [0_u8; 32];
    for (index, chunk) in trimmed.as_bytes().chunks_exact(2).enumerate() {
        let hi = hex_value(chunk[0])?;
        let lo = hex_value(chunk[1])?;
        out[index] = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn to_current_user(projection: &UserProjection) -> CurrentUser {
    CurrentUser {
        id: projection.id.clone(),
        tenant: TenantSummary {
            id: projection.tenant_id.clone(),
            display_name: projection.tenant_display_name.clone(),
            base_currency: projection.tenant_base_currency.clone(),
            state: projection.tenant_state.clone(),
        },
        status: map_status(&projection.status),
        display_name: projection.display_name.clone(),
        email: projection.email.clone(),
        roles: projection
            .roles
            .iter()
            .filter_map(|role| map_role(role))
            .collect(),
        linked_providers: projection
            .providers
            .iter()
            .filter_map(|provider| Provider::from_db(provider))
            .collect(),
        mfa_enrolled: projection.mfa_enrolled,
    }
}

fn map_status(value: &str) -> UserStatus {
    match value {
        "ACTIVE" => UserStatus::Active,
        "SUSPENDED" => UserStatus::Suspended,
        "DELETED" => UserStatus::Deleted,
        _ => UserStatus::Pending,
    }
}

fn map_role(value: &str) -> Option<Role> {
    match value {
        "VIEWER" => Some(Role::Viewer),
        "OWNER" => Some(Role::Owner),
        "TRADER" => Some(Role::Trader),
        "APPROVER" => Some(Role::Approver),
        _ => None,
    }
}
