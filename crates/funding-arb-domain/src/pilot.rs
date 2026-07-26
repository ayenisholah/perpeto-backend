//! Tenant pilot-arming state machine (M4 Slice 7).
//!
//! Mirrors the `tenant_state` database enum and owns the only legal transitions
//! between paper and armed pilot operation. It is pure: the API composes it with
//! passkey authorization, a typed confirmation, and per-account preflight
//! eligibility, then persists the resulting state. Arming raises risk and is
//! passkey-gated at the transport layer; disarming lowers risk and is always
//! reachable. LIVE order execution is a later slice — an armed tenant only
//! records intent here.

/// The lifecycle state of a tenant with respect to live-pilot trading.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PilotState {
    PaperOnly,
    PilotEligible,
    PilotArmed,
    Suspended,
    Deleted,
}

impl PilotState {
    /// Parse the database `tenant_state` text form.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "PAPER_ONLY" => Some(Self::PaperOnly),
            "PILOT_ELIGIBLE" => Some(Self::PilotEligible),
            "PILOT_ARMED" => Some(Self::PilotArmed),
            "SUSPENDED" => Some(Self::Suspended),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }

    /// The database `tenant_state` text form.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::PaperOnly => "PAPER_ONLY",
            Self::PilotEligible => "PILOT_ELIGIBLE",
            Self::PilotArmed => "PILOT_ARMED",
            Self::Suspended => "SUSPENDED",
            Self::Deleted => "DELETED",
        }
    }

    /// Whether the tenant is armed for pilot trading (records intent only until
    /// LIVE execution is unlocked in a later slice).
    #[must_use]
    pub const fn is_armed(self) -> bool {
        matches!(self, Self::PilotArmed)
    }

    /// Promote a paper tenant to pilot-eligible once at least one venue account
    /// has passed preflight.
    ///
    /// # Errors
    /// Returns [`PilotError`] unless the tenant is `PAPER_ONLY` with an eligible
    /// account.
    pub fn grant_eligibility(self, has_eligible_account: bool) -> Result<Self, PilotError> {
        self.available()?;
        match self {
            Self::PaperOnly if has_eligible_account => Ok(Self::PilotEligible),
            Self::PaperOnly => Err(PilotError::NoEligibleAccount),
            Self::PilotEligible | Self::PilotArmed => Err(PilotError::NotPaperOnly),
            Self::Suspended | Self::Deleted => Err(PilotError::TenantUnavailable),
        }
    }

    /// Arm an eligible tenant. Both an eligible account and a matching typed
    /// confirmation are required.
    ///
    /// # Errors
    /// Returns [`PilotError`] unless the tenant is `PILOT_ELIGIBLE`, the account
    /// is eligible, and the confirmation matched.
    pub fn arm(self, account_eligible: bool, confirmation_ok: bool) -> Result<Self, PilotError> {
        self.available()?;
        match self {
            Self::PilotEligible => {
                if !account_eligible {
                    return Err(PilotError::AccountNotEligible);
                }
                if !confirmation_ok {
                    return Err(PilotError::ConfirmationRequired);
                }
                Ok(Self::PilotArmed)
            }
            Self::PaperOnly => Err(PilotError::NotEligible),
            Self::PilotArmed => Err(PilotError::NotEligible),
            Self::Suspended | Self::Deleted => Err(PilotError::TenantUnavailable),
        }
    }

    /// Disarm an armed tenant back to eligible. Always reachable for an available
    /// tenant so operators can reach safety without friction.
    ///
    /// # Errors
    /// Returns [`PilotError::NotArmed`] when the tenant is not armed.
    pub fn disarm(self) -> Result<Self, PilotError> {
        self.available()?;
        match self {
            Self::PilotArmed => Ok(Self::PilotEligible),
            _ => Err(PilotError::NotArmed),
        }
    }

    /// Reject transitions on suspended or deleted tenants up front.
    fn available(self) -> Result<(), PilotError> {
        match self {
            Self::Suspended | Self::Deleted => Err(PilotError::TenantUnavailable),
            _ => Ok(()),
        }
    }
}

/// Why a pilot transition was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PilotError {
    NotPaperOnly,
    NoEligibleAccount,
    NotEligible,
    AccountNotEligible,
    ConfirmationRequired,
    NotArmed,
    TenantUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_forms_round_trip() {
        for state in [
            PilotState::PaperOnly,
            PilotState::PilotEligible,
            PilotState::PilotArmed,
            PilotState::Suspended,
            PilotState::Deleted,
        ] {
            assert_eq!(PilotState::from_wire(state.wire()), Some(state));
        }
        assert_eq!(PilotState::from_wire("NONSENSE"), None);
    }

    #[test]
    fn eligibility_requires_paper_and_an_eligible_account() {
        assert_eq!(
            PilotState::PaperOnly.grant_eligibility(true),
            Ok(PilotState::PilotEligible)
        );
        assert_eq!(
            PilotState::PaperOnly.grant_eligibility(false),
            Err(PilotError::NoEligibleAccount)
        );
        assert_eq!(
            PilotState::PilotEligible.grant_eligibility(true),
            Err(PilotError::NotPaperOnly)
        );
    }

    #[test]
    fn arming_requires_eligible_state_account_and_confirmation() {
        assert_eq!(
            PilotState::PilotEligible.arm(true, true),
            Ok(PilotState::PilotArmed)
        );
        assert_eq!(
            PilotState::PilotEligible.arm(false, true),
            Err(PilotError::AccountNotEligible)
        );
        assert_eq!(
            PilotState::PilotEligible.arm(true, false),
            Err(PilotError::ConfirmationRequired)
        );
        // Cannot arm straight from paper or re-arm.
        assert_eq!(
            PilotState::PaperOnly.arm(true, true),
            Err(PilotError::NotEligible)
        );
        assert_eq!(
            PilotState::PilotArmed.arm(true, true),
            Err(PilotError::NotEligible)
        );
    }

    #[test]
    fn disarm_only_from_armed() {
        assert_eq!(
            PilotState::PilotArmed.disarm(),
            Ok(PilotState::PilotEligible)
        );
        assert_eq!(
            PilotState::PilotEligible.disarm(),
            Err(PilotError::NotArmed)
        );
    }

    #[test]
    fn suspended_and_deleted_refuse_every_transition() {
        for state in [PilotState::Suspended, PilotState::Deleted] {
            assert_eq!(
                state.grant_eligibility(true),
                Err(PilotError::TenantUnavailable)
            );
            assert_eq!(state.arm(true, true), Err(PilotError::TenantUnavailable));
            assert_eq!(state.disarm(), Err(PilotError::TenantUnavailable));
        }
    }
}
