//! Serialized venue-account and exact instrument DTOs.

use serde::{Deserialize, Serialize};

/// Static capability summary plus configured-account count.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VenueSummaryDto {
    pub venue: String,
    pub display_name: String,
    pub products: Vec<String>,
    pub sandbox_environment: String,
    pub configured_accounts: usize,
}

/// Non-invasive configuration test. It never submits or cancels orders.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionTestDto {
    pub account_id: String,
    pub status: String,
    pub checked_at: String,
    pub checks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectorOrderDto {
    pub id: String,
    pub venue: String,
    pub venue_account_id: String,
    pub client_order_id: String,
    pub venue_order_id: Option<String>,
    pub symbol: String,
    pub product: String,
    pub side: String,
    pub base_quantity: String,
    pub limit_price: String,
    pub native_quantity: String,
    pub native_price: String,
    pub equivalent_base_quantity: String,
    pub projected_delta_usd: String,
    pub reduce_only: bool,
    pub environment: String,
    pub state: String,
    pub payload_hash: String,
    pub error_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReconciliationRunDto {
    pub id: String,
    pub venue: String,
    pub venue_account_id: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub balances_seen: i32,
    pub positions_seen: i32,
    pub open_orders_seen: i32,
    pub fills_seen: i32,
    pub discrepancies: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExactBalanceDto {
    pub asset: String,
    pub total: String,
    pub available: String,
    pub locked: String,
    pub received_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExactVenuePositionDto {
    pub symbol: String,
    pub product: String,
    pub quantity: String,
    pub entry_price: String,
    pub mark_price: String,
    pub leverage: String,
    pub liquidation_price: Option<String>,
    pub received_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectorAccountingDto {
    pub venue_account_id: String,
    pub run_id: Option<String>,
    pub balances: Vec<ExactBalanceDto>,
    pub positions: Vec<ExactVenuePositionDto>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CertificationState {
    Disabled,
    PaperOnly,
    Shadow,
    Pilot,
    CertifiedLive,
}

/// Why a certification promotion was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CertificationError {
    IllegalTransition,
}

impl CertificationState {
    /// Stable text form matching the wire enum.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Disabled => "DISABLED",
            Self::PaperOnly => "PAPER_ONLY",
            Self::Shadow => "SHADOW",
            Self::Pilot => "PILOT",
            Self::CertifiedLive => "CERTIFIED_LIVE",
        }
    }

    /// Advance certification by exactly one legal step, or disable from any
    /// state. Skips and backward moves are rejected; `Disabled` is terminal for
    /// promotion (re-enrollment is a separate flow).
    ///
    /// # Errors
    /// Returns [`CertificationError::IllegalTransition`] for any other move.
    pub fn promote(self, target: Self) -> Result<Self, CertificationError> {
        if target == Self::Disabled {
            return Ok(Self::Disabled);
        }
        let legal = matches!(
            (self, target),
            (Self::PaperOnly, Self::Shadow)
                | (Self::Shadow, Self::Pilot)
                | (Self::Pilot, Self::CertifiedLive)
        );
        if legal {
            Ok(target)
        } else {
            Err(CertificationError::IllegalTransition)
        }
    }
}

#[cfg(test)]
mod certification_tests {
    use super::{CertificationError, CertificationState};

    #[test]
    fn promotion_advances_one_legal_step() {
        assert_eq!(
            CertificationState::PaperOnly.promote(CertificationState::Shadow),
            Ok(CertificationState::Shadow)
        );
        assert_eq!(
            CertificationState::Shadow.promote(CertificationState::Pilot),
            Ok(CertificationState::Pilot)
        );
        assert_eq!(
            CertificationState::Pilot.promote(CertificationState::CertifiedLive),
            Ok(CertificationState::CertifiedLive)
        );
    }

    #[test]
    fn promotion_rejects_skips_and_reverses() {
        assert_eq!(
            CertificationState::PaperOnly.promote(CertificationState::Pilot),
            Err(CertificationError::IllegalTransition)
        );
        assert_eq!(
            CertificationState::Pilot.promote(CertificationState::Shadow),
            Err(CertificationError::IllegalTransition)
        );
        assert_eq!(
            CertificationState::PaperOnly.promote(CertificationState::PaperOnly),
            Err(CertificationError::IllegalTransition)
        );
    }

    #[test]
    fn any_state_can_be_disabled_but_disabled_is_terminal() {
        assert_eq!(
            CertificationState::Pilot.promote(CertificationState::Disabled),
            Ok(CertificationState::Disabled)
        );
        assert_eq!(
            CertificationState::Disabled.promote(CertificationState::Shadow),
            Err(CertificationError::IllegalTransition)
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReconciliationState {
    Pending,
    Reconciled,
    Required,
    Frozen,
}

/// One product/environment connection grouped under a logical venue account.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProductConnectionDto {
    pub id: String,
    pub product: String,
    pub environment: String,
    pub secret_ref: Option<String>,
    pub read_permission: bool,
    pub trade_permission: bool,
    pub withdrawal_permission: Option<bool>,
    pub status: String,
    pub last_tested_at: Option<String>,
}

/// Masked account resource. It never contains credential material.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VenueAccountDto {
    pub id: String,
    pub venue: String,
    pub alias: String,
    pub masked_identity: String,
    pub enabled: bool,
    pub certification_state: CertificationState,
    pub reconciliation_state: ReconciliationState,
    pub connections: Vec<ProductConnectionDto>,
    pub last_reconciled_at: Option<String>,
    pub created_at: String,
    pub tombstoned_at: Option<String>,
}

/// Exact venue instrument read model. Decimal values remain strings on the wire.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstrumentDto {
    pub venue: String,
    pub symbol: String,
    pub product: String,
    pub canonical_underlying: String,
    pub quote_asset: String,
    pub settlement_asset: String,
    pub margin_asset: String,
    pub contract_multiplier: String,
    pub price_tick: String,
    pub quantity_step: String,
    pub minimum_quantity: String,
    pub maximum_quantity: Option<String>,
    pub minimum_notional: String,
    pub funding_interval_hours: Option<u32>,
    pub funding_cap: Option<String>,
    pub funding_floor: Option<String>,
    pub maximum_leverage: Option<String>,
    pub trading: bool,
    pub metadata_version: String,
    pub venue_updated_at: Option<String>,
    pub received_at: String,
}
