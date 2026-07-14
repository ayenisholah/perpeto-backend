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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthInvariantError {
    BootstrapAlreadyConsumed,
    ChallengeAlreadyConsumed,
    ChallengeExpired,
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
}
