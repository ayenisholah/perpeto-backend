//! Authentication invariants shared by the API and persistence adapters.
//!
//! Provider token verification, encryption, hashing, and database transactions
//! belong to adapters. This module owns the state transitions that must remain
//! true regardless of provider or transport.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SocialProvider {
    Apple,
    Google,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserStatus {
    Pending,
    Active,
    Suspended,
    Deleted,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Role {
    Viewer,
    Owner,
    Trader,
    Approver,
}

impl Role {
    #[must_use]
    pub const fn requires_mfa(self) -> bool {
        matches!(self, Self::Owner | Self::Trader | Self::Approver)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthNextState {
    PendingApproval,
    MfaRequired,
    MfaEnrollmentRequired,
    Authenticated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderIdentity {
    pub provider: SocialProvider,
    pub provider_subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub id: String,
    pub status: UserStatus,
    pub roles: BTreeSet<Role>,
    pub identities: BTreeMap<SocialProvider, ProviderIdentity>,
    pub mfa_confirmed: bool,
    pub recovery_codes_acknowledged: bool,
}

impl User {
    #[must_use]
    pub fn next_state(&self) -> AuthNextState {
        if self.status != UserStatus::Active {
            return AuthNextState::PendingApproval;
        }
        if self.roles.iter().any(|role| role.requires_mfa()) {
            if !self.mfa_confirmed || !self.recovery_codes_acknowledged {
                return AuthNextState::MfaEnrollmentRequired;
            }
            return AuthNextState::MfaRequired;
        }
        AuthNextState::Authenticated
    }

    pub fn approve_as_viewer(&mut self) -> Result<(), AuthInvariantError> {
        if self.status != UserStatus::Pending {
            return Err(AuthInvariantError::UserIsNotPending);
        }
        self.status = UserStatus::Active;
        self.roles.clear();
        self.roles.insert(Role::Viewer);
        Ok(())
    }

    /// Admit a pending user to full access under open registration (DEC-0009).
    ///
    /// Grants every privileged role. The mandatory-MFA rule in
    /// [`Self::next_state`] still applies; open-registration deployments waive
    /// it at the service policy layer, not here.
    pub fn admit_with_full_access(&mut self) -> Result<(), AuthInvariantError> {
        if self.status != UserStatus::Pending {
            return Err(AuthInvariantError::UserIsNotPending);
        }
        self.status = UserStatus::Active;
        self.roles.clear();
        self.roles.insert(Role::Owner);
        self.roles.insert(Role::Trader);
        self.roles.insert(Role::Approver);
        Ok(())
    }

    pub fn assign_privileged_role(&mut self, role: Role) -> Result<(), AuthInvariantError> {
        if !role.requires_mfa() {
            return Err(AuthInvariantError::RoleIsNotPrivileged);
        }
        if self.status != UserStatus::Active {
            return Err(AuthInvariantError::UserIsNotActive);
        }
        self.roles.insert(role);
        Ok(())
    }

    pub fn link_identity(
        &mut self,
        identity: ProviderIdentity,
        fresh_provider_authentication: bool,
        totp_verified: bool,
    ) -> Result<(), AuthInvariantError> {
        if !fresh_provider_authentication {
            return Err(AuthInvariantError::FreshProviderAuthenticationRequired);
        }
        if self.roles.iter().any(|role| role.requires_mfa()) && !totp_verified {
            return Err(AuthInvariantError::TotpRequired);
        }
        if self.identities.contains_key(&identity.provider) {
            return Err(AuthInvariantError::ProviderAlreadyLinked);
        }
        self.identities.insert(identity.provider, identity);
        Ok(())
    }

    pub fn unlink_identity(&mut self, provider: SocialProvider) -> Result<(), AuthInvariantError> {
        if !self.identities.contains_key(&provider) {
            return Err(AuthInvariantError::ProviderNotLinked);
        }
        if self.identities.len() == 1 {
            return Err(AuthInvariantError::FinalIdentityCannotBeUnlinked);
        }
        self.identities.remove(&provider);
        Ok(())
    }

    pub fn delete(&mut self, active_owner_count: usize) -> Result<(), AuthInvariantError> {
        if self.roles.contains(&Role::Owner) && active_owner_count <= 1 {
            return Err(AuthInvariantError::FinalOwnerCannotBeDeleted);
        }
        self.status = UserStatus::Deleted;
        self.roles.clear();
        self.identities.clear();
        self.mfa_confirmed = false;
        self.recovery_codes_acknowledged = false;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OneUseChallenge {
    pub expires_at_epoch_seconds: u64,
    consumed: bool,
}

impl OneUseChallenge {
    #[must_use]
    pub const fn new(expires_at_epoch_seconds: u64) -> Self {
        Self {
            expires_at_epoch_seconds,
            consumed: false,
        }
    }

    pub fn consume(&mut self, now_epoch_seconds: u64) -> Result<(), AuthInvariantError> {
        if self.consumed {
            return Err(AuthInvariantError::ChallengeAlreadyConsumed);
        }
        if now_epoch_seconds >= self.expires_at_epoch_seconds {
            return Err(AuthInvariantError::ChallengeExpired);
        }
        self.consumed = true;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapState {
    consumed: bool,
}

impl BootstrapState {
    #[must_use]
    pub const fn available() -> Self {
        Self { consumed: false }
    }

    pub fn consume(
        &mut self,
        user: &mut User,
        deployment_token_valid: bool,
    ) -> Result<(), AuthInvariantError> {
        if self.consumed {
            return Err(AuthInvariantError::BootstrapAlreadyConsumed);
        }
        if !deployment_token_valid || user.identities.is_empty() {
            return Err(AuthInvariantError::InvalidBootstrapProof);
        }
        user.status = UserStatus::Active;
        user.roles.clear();
        user.roles.insert(Role::Owner);
        self.consumed = true;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Active,
    Revoked,
    Compromised,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshSession {
    pub family_id: String,
    pub device_id: String,
    pub state: SessionState,
    generation: u64,
    current_token_hash: [u8; 32],
    used_token_hashes: BTreeSet<[u8; 32]>,
}

impl RefreshSession {
    #[must_use]
    pub fn new(family_id: String, device_id: String, token_hash: [u8; 32]) -> Self {
        Self {
            family_id,
            device_id,
            state: SessionState::Active,
            generation: 0,
            current_token_hash: token_hash,
            used_token_hashes: BTreeSet::new(),
        }
    }

    pub fn rotate(
        &mut self,
        device_id: &str,
        presented_token_hash: [u8; 32],
        next_token_hash: [u8; 32],
    ) -> Result<u64, AuthInvariantError> {
        if self.state != SessionState::Active {
            return Err(AuthInvariantError::SessionInactive);
        }
        if self.device_id != device_id {
            return Err(AuthInvariantError::RefreshTokenDeviceMismatch);
        }
        if self.used_token_hashes.contains(&presented_token_hash) {
            self.state = SessionState::Compromised;
            return Err(AuthInvariantError::RefreshTokenReuse);
        }
        if self.current_token_hash != presented_token_hash {
            return Err(AuthInvariantError::InvalidRefreshToken);
        }
        self.used_token_hashes.insert(presented_token_hash);
        self.current_token_hash = next_token_hash;
        self.generation += 1;
        Ok(self.generation)
    }

    pub fn revoke(&mut self) {
        self.state = SessionState::Revoked;
    }
}

/// A WebAuthn ceremony kind. Registration binds a new authenticator;
/// authentication asserts possession of an already-registered one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ceremony {
    Register,
    Authenticate,
}

/// Opaque WebAuthn credential identifier, unique within the relying party.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CredentialId(Vec<u8>);

impl CredentialId {
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Monotonic authenticator signature counter. A presented value that does not
/// exceed the stored one indicates a cloned authenticator and must be rejected;
/// a value of zero from a counter-less authenticator stays at zero forever.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignCount(u32);

impl SignCount {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }

    /// Advance to a freshly asserted counter. Counter-less authenticators keep
    /// both values at zero; any other non-increase is a clone signal.
    pub fn advance(&mut self, presented: SignCount) -> Result<(), AuthInvariantError> {
        if self.0 == 0 && presented.0 == 0 {
            return Ok(());
        }
        if presented.0 <= self.0 {
            return Err(AuthInvariantError::SignCountRegression);
        }
        self.0 = presented.0;
        Ok(())
    }
}

/// The exact action a passkey assertion authorizes. A proof minted for one
/// descriptor can never satisfy another. Marked `#[non_exhaustive]`: later M4
/// slices add credential-enrollment and pilot-arming variants without breaking
/// existing matches.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ActionDescriptor {
    RegisterPasskey,
    RevokePasskey {
        credential_id: CredentialId,
    },
    /// Enroll a new exchange credential (creating the account). A new account has
    /// no id yet, so the natural key is bound.
    EnrollCredential {
        venue: String,
        alias: String,
        product: String,
        environment: String,
    },
    RotateCredential {
        venue_account_id: String,
        venue: String,
        product: String,
    },
    RevokeCredential {
        venue_account_id: String,
        venue: String,
        product: String,
    },
    /// Delete a venue account entirely (destroying every connection's secret and
    /// tombstoning the account). Account-level, so it binds no product.
    DeleteCredential {
        venue_account_id: String,
        venue: String,
    },
    /// Arm a tenant for pilot trading against one preflight-eligible route.
    ArmPilot {
        venue_account_id: String,
        product: String,
    },
    /// Promote a venue account's platform certification (e.g. to `PILOT`).
    CertifyRoute {
        venue_account_id: String,
        target: String,
    },
}

impl ActionDescriptor {
    /// Stable, injective encoding used as the binding key persisted alongside a
    /// challenge and compared when a proof is consumed. Two descriptors are the
    /// same action iff their canonical forms are equal.
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::RegisterPasskey => "register_passkey".to_owned(),
            Self::RevokePasskey { credential_id } => {
                let mut encoded = String::from("revoke_passkey:");
                for byte in credential_id.as_bytes() {
                    use std::fmt::Write as _;
                    let _ = write!(encoded, "{byte:02x}");
                }
                encoded
            }
            Self::EnrollCredential {
                venue,
                alias,
                product,
                environment,
            } => length_prefixed("enroll_credential", &[venue, alias, product, environment]),
            Self::RotateCredential {
                venue_account_id,
                venue,
                product,
            } => length_prefixed("rotate_credential", &[venue_account_id, venue, product]),
            Self::RevokeCredential {
                venue_account_id,
                venue,
                product,
            } => length_prefixed("revoke_credential", &[venue_account_id, venue, product]),
            Self::DeleteCredential {
                venue_account_id,
                venue,
            } => length_prefixed("delete_credential", &[venue_account_id, venue]),
            Self::ArmPilot {
                venue_account_id,
                product,
            } => length_prefixed("arm_pilot", &[venue_account_id, product]),
            Self::CertifyRoute {
                venue_account_id,
                target,
            } => length_prefixed("certify_route", &[venue_account_id, target]),
        }
    }
}

/// Injective `tag:len:value:len:value...` encoding. The length prefixes make the
/// result unambiguous even when a field (e.g. a user-chosen alias) contains the
/// delimiter, so distinct field tuples never collide.
fn length_prefixed(tag: &str, parts: &[&str]) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::from(tag);
    for part in parts {
        let _ = write!(encoded, ":{}:{part}", part.len());
    }
    encoded
}

/// A single-use authorization proof bound to one action. Verifying a passkey
/// assertion for action A yields a proof that consumes exactly once and only
/// authorizes A.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionProof {
    action: ActionDescriptor,
    expires_at_epoch_seconds: u64,
    consumed: bool,
}

impl ActionProof {
    #[must_use]
    pub const fn new(action: ActionDescriptor, expires_at_epoch_seconds: u64) -> Self {
        Self {
            action,
            expires_at_epoch_seconds,
            consumed: false,
        }
    }

    /// Consume the proof to authorize `requested`. Fails closed on mismatch,
    /// reuse, or expiry.
    pub fn authorize(
        &mut self,
        requested: &ActionDescriptor,
        now_epoch_seconds: u64,
    ) -> Result<(), AuthInvariantError> {
        if self.consumed {
            return Err(AuthInvariantError::ActionProofAlreadyConsumed);
        }
        if now_epoch_seconds >= self.expires_at_epoch_seconds {
            return Err(AuthInvariantError::ActionProofExpired);
        }
        if self.action.canonical() != requested.canonical() {
            return Err(AuthInvariantError::ActionProofMismatch);
        }
        self.consumed = true;
        Ok(())
    }
}

/// A registered passkey. Owns the anti-clone signature-count invariant and the
/// revocation lifecycle; the actual assertion cryptography lives in the adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasskeyCredential {
    pub credential_id: CredentialId,
    sign_count: SignCount,
    revoked: bool,
}

impl PasskeyCredential {
    #[must_use]
    pub const fn new(credential_id: CredentialId, sign_count: SignCount) -> Self {
        Self {
            credential_id,
            sign_count,
            revoked: false,
        }
    }

    #[must_use]
    pub const fn sign_count(&self) -> SignCount {
        self.sign_count
    }

    #[must_use]
    pub const fn is_revoked(&self) -> bool {
        self.revoked
    }

    /// Record a verified assertion, advancing the signature counter. Rejects a
    /// revoked credential or a non-increasing counter (clone signal).
    pub fn record_assertion(&mut self, presented: SignCount) -> Result<(), AuthInvariantError> {
        if self.revoked {
            return Err(AuthInvariantError::CredentialRevoked);
        }
        self.sign_count.advance(presented)
    }

    pub fn revoke(&mut self) {
        self.revoked = true;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthInvariantError {
    ActionProofAlreadyConsumed,
    ActionProofExpired,
    ActionProofMismatch,
    BootstrapAlreadyConsumed,
    ChallengeAlreadyConsumed,
    ChallengeExpired,
    CredentialRevoked,
    FinalIdentityCannotBeUnlinked,
    FinalOwnerCannotBeDeleted,
    FreshProviderAuthenticationRequired,
    InvalidBootstrapProof,
    InvalidRefreshToken,
    ProviderAlreadyLinked,
    ProviderNotLinked,
    RefreshTokenDeviceMismatch,
    RefreshTokenReuse,
    RoleIsNotPrivileged,
    SessionInactive,
    SignCountRegression,
    TotpRequired,
    UserIsNotActive,
    UserIsNotPending,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn social_user(status: UserStatus) -> User {
        let identity = ProviderIdentity {
            provider: SocialProvider::Google,
            provider_subject: String::from("immutable-google-sub"),
            email: Some(String::from("mutable@example.com")),
            display_name: Some(String::from("Operator")),
        };
        User {
            id: String::from("user-1"),
            status,
            roles: BTreeSet::new(),
            identities: BTreeMap::from([(SocialProvider::Google, identity)]),
            mfa_confirmed: false,
            recovery_codes_acknowledged: false,
        }
    }

    #[test]
    fn new_social_users_are_pending_until_owner_approval() {
        let mut user = social_user(UserStatus::Pending);
        assert_eq!(user.next_state(), AuthNextState::PendingApproval);
        user.approve_as_viewer().unwrap();
        assert_eq!(user.roles, BTreeSet::from([Role::Viewer]));
        assert_eq!(user.next_state(), AuthNextState::Authenticated);
    }

    #[test]
    fn open_registration_admits_pending_user_to_all_privileged_roles() {
        let mut user = social_user(UserStatus::Pending);
        user.admit_with_full_access().unwrap();
        assert_eq!(user.status, UserStatus::Active);
        assert_eq!(
            user.roles,
            BTreeSet::from([Role::Owner, Role::Trader, Role::Approver])
        );
    }

    #[test]
    fn privileged_roles_require_completed_mfa_enrollment_then_mfa() {
        let mut user = social_user(UserStatus::Active);
        user.assign_privileged_role(Role::Trader).unwrap();
        assert_eq!(user.next_state(), AuthNextState::MfaEnrollmentRequired);
        user.mfa_confirmed = true;
        user.recovery_codes_acknowledged = true;
        assert_eq!(user.next_state(), AuthNextState::MfaRequired);
    }

    #[test]
    fn bootstrap_is_one_use_and_requires_a_social_identity() {
        let mut bootstrap = BootstrapState::available();
        let mut first = social_user(UserStatus::Pending);
        bootstrap.consume(&mut first, true).unwrap();
        assert_eq!(first.roles, BTreeSet::from([Role::Owner]));
        assert_eq!(
            bootstrap.consume(&mut social_user(UserStatus::Pending), true),
            Err(AuthInvariantError::BootstrapAlreadyConsumed)
        );
    }

    #[test]
    fn identity_linking_never_uses_email_and_preserves_a_final_provider() {
        let mut user = social_user(UserStatus::Active);
        let apple = ProviderIdentity {
            provider: SocialProvider::Apple,
            provider_subject: String::from("apple-sub"),
            email: Some(String::from("mutable@example.com")),
            display_name: None,
        };
        user.link_identity(apple, true, false).unwrap();
        user.unlink_identity(SocialProvider::Google).unwrap();
        assert_eq!(
            user.unlink_identity(SocialProvider::Apple),
            Err(AuthInvariantError::FinalIdentityCannotBeUnlinked)
        );
    }

    #[test]
    fn privileged_identity_linking_requires_totp() {
        let mut user = social_user(UserStatus::Active);
        user.roles.insert(Role::Owner);
        let apple = ProviderIdentity {
            provider: SocialProvider::Apple,
            provider_subject: String::from("apple-sub"),
            email: None,
            display_name: None,
        };
        assert_eq!(
            user.link_identity(apple.clone(), true, false),
            Err(AuthInvariantError::TotpRequired)
        );
        user.link_identity(apple, true, true).unwrap();
    }

    #[test]
    fn refresh_rotation_detects_reuse_and_compromises_family() {
        let mut session =
            RefreshSession::new(String::from("family"), String::from("phone"), [1; 32]);
        assert_eq!(session.rotate("phone", [1; 32], [2; 32]), Ok(1));
        assert_eq!(
            session.rotate("phone", [1; 32], [3; 32]),
            Err(AuthInvariantError::RefreshTokenReuse)
        );
        assert_eq!(session.state, SessionState::Compromised);
    }

    #[test]
    fn final_owner_cannot_be_deleted() {
        let mut user = social_user(UserStatus::Active);
        user.roles.insert(Role::Owner);
        assert_eq!(
            user.delete(1),
            Err(AuthInvariantError::FinalOwnerCannotBeDeleted)
        );
        user.delete(2).unwrap();
        assert_eq!(user.status, UserStatus::Deleted);
        assert!(user.identities.is_empty());
    }

    #[test]
    fn challenge_is_expiring_and_single_use() {
        let mut challenge = OneUseChallenge::new(100);
        challenge.consume(99).unwrap();
        assert_eq!(
            challenge.consume(99),
            Err(AuthInvariantError::ChallengeAlreadyConsumed)
        );
    }

    #[test]
    fn sign_count_regression_signals_a_cloned_authenticator() {
        let mut counter = SignCount::new(7);
        counter.advance(SignCount::new(8)).unwrap();
        assert_eq!(counter.value(), 8);
        assert_eq!(
            counter.advance(SignCount::new(8)),
            Err(AuthInvariantError::SignCountRegression)
        );
        // Counter-less authenticators legitimately stay at zero.
        let mut counterless = SignCount::new(0);
        counterless.advance(SignCount::new(0)).unwrap();
    }

    #[test]
    fn passkey_rejects_assertions_after_revocation() {
        let mut credential =
            PasskeyCredential::new(CredentialId::new(vec![1, 2, 3]), SignCount::new(1));
        credential.record_assertion(SignCount::new(2)).unwrap();
        credential.revoke();
        assert_eq!(
            credential.record_assertion(SignCount::new(3)),
            Err(AuthInvariantError::CredentialRevoked)
        );
    }

    #[test]
    fn action_proof_authorizes_exactly_its_bound_action_once() {
        let bound = ActionDescriptor::RevokePasskey {
            credential_id: CredentialId::new(vec![9, 9]),
        };
        let mut proof = ActionProof::new(bound.clone(), 100);
        // A proof for one action never satisfies another.
        assert_eq!(
            proof.authorize(&ActionDescriptor::RegisterPasskey, 50),
            Err(AuthInvariantError::ActionProofMismatch)
        );
        proof.authorize(&bound, 50).unwrap();
        assert_eq!(
            proof.authorize(&bound, 50),
            Err(AuthInvariantError::ActionProofAlreadyConsumed)
        );
    }

    #[test]
    fn action_proof_expires() {
        let mut proof = ActionProof::new(ActionDescriptor::RegisterPasskey, 100);
        assert_eq!(
            proof.authorize(&ActionDescriptor::RegisterPasskey, 100),
            Err(AuthInvariantError::ActionProofExpired)
        );
    }

    #[test]
    fn action_descriptor_canonical_form_is_injective() {
        let a = ActionDescriptor::RevokePasskey {
            credential_id: CredentialId::new(vec![0x0a, 0xff]),
        };
        let b = ActionDescriptor::RevokePasskey {
            credential_id: CredentialId::new(vec![0xff, 0x0a]),
        };
        assert_ne!(a.canonical(), b.canonical());
        assert_ne!(a.canonical(), ActionDescriptor::RegisterPasskey.canonical());
    }

    #[test]
    fn credential_action_descriptors_are_distinct_and_injective() {
        let enroll = ActionDescriptor::EnrollCredential {
            venue: "BYBIT".to_owned(),
            alias: "primary".to_owned(),
            product: "LINEAR_PERPETUAL".to_owned(),
            environment: "PRODUCTION".to_owned(),
        };
        let rotate = ActionDescriptor::RotateCredential {
            venue_account_id: "acct-1".to_owned(),
            venue: "BYBIT".to_owned(),
            product: "LINEAR_PERPETUAL".to_owned(),
        };
        let revoke = ActionDescriptor::RevokeCredential {
            venue_account_id: "acct-1".to_owned(),
            venue: "BYBIT".to_owned(),
            product: "LINEAR_PERPETUAL".to_owned(),
        };
        let arm = ActionDescriptor::ArmPilot {
            venue_account_id: "acct-1".to_owned(),
            product: "LINEAR_PERPETUAL".to_owned(),
        };
        let certify = ActionDescriptor::CertifyRoute {
            venue_account_id: "acct-1".to_owned(),
            target: "PILOT".to_owned(),
        };
        let delete = ActionDescriptor::DeleteCredential {
            venue_account_id: "acct-1".to_owned(),
            venue: "BYBIT".to_owned(),
        };
        // The credential actions, pilot arming/certification, and passkey actions are distinct.
        let forms = [
            enroll.canonical(),
            rotate.canonical(),
            revoke.canonical(),
            delete.canonical(),
            arm.canonical(),
            certify.canonical(),
            ActionDescriptor::RegisterPasskey.canonical(),
        ];
        for (i, left) in forms.iter().enumerate() {
            for right in &forms[i + 1..] {
                assert_ne!(left, right);
            }
        }

        // Length-prefixing keeps distinct field tuples from colliding even when a
        // field contains the ':' delimiter.
        let tricky_a = ActionDescriptor::EnrollCredential {
            venue: "A".to_owned(),
            alias: "x:y".to_owned(),
            product: "P".to_owned(),
            environment: "E".to_owned(),
        };
        let tricky_b = ActionDescriptor::EnrollCredential {
            venue: "A".to_owned(),
            alias: "x".to_owned(),
            product: "y:P".to_owned(),
            environment: "E".to_owned(),
        };
        assert_ne!(tricky_a.canonical(), tricky_b.canonical());
    }
}
