//! Serialized venue-account and exact instrument DTOs.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CertificationState {
    Disabled,
    PaperOnly,
    Shadow,
    Pilot,
    CertifiedLive,
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
