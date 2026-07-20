//! Venue-neutral connector value types.

use chrono::{DateTime, Utc};
use funding_arb_domain::market::Venue;
use rust_decimal::Decimal;

/// Network environment attached to one credential profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VenueEnvironment {
    Production,
    Testnet,
    Demo,
}

impl VenueEnvironment {
    /// Whether venue writes are guaranteed to use simulated funds.
    #[must_use]
    pub const fn is_non_production(self) -> bool {
        matches!(self, Self::Testnet | Self::Demo)
    }

    /// Canonical wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Production => "PRODUCTION",
            Self::Testnet => "TESTNET",
            Self::Demo => "DEMO",
        }
    }
}

/// Supported M3 product family.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProductType {
    Spot,
    LinearPerpetual,
}

impl ProductType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spot => "SPOT",
            Self::LinearPerpetual => "LINEAR_PERPETUAL",
        }
    }
}

/// Connector capabilities are advertised, never inferred from venue name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenueCapabilities {
    pub products: Vec<ProductType>,
    pub market_stream: bool,
    pub private_stream: bool,
    pub funding_history: bool,
    pub order_submission: bool,
    pub borrow: bool,
    pub internal_transfer: bool,
}

/// Stable account identifier owned by Perpeto.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AccountId(pub String);

/// Stable venue-native instrument identity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstrumentId {
    pub venue: Venue,
    pub symbol: String,
    pub product: ProductType,
}

/// Exact dynamic instrument metadata.
#[derive(Clone, Debug)]
pub struct Instrument {
    pub id: InstrumentId,
    pub canonical_underlying: String,
    pub quote_asset: String,
    pub settlement_asset: String,
    pub margin_asset: String,
    pub contract_multiplier: Decimal,
    pub price_tick: Decimal,
    pub quantity_step: Decimal,
    pub minimum_quantity: Decimal,
    pub maximum_quantity: Option<Decimal>,
    pub minimum_notional: Decimal,
    pub funding_interval_hours: Option<u32>,
    pub funding_cap: Option<Decimal>,
    pub funding_floor: Option<Decimal>,
    pub maximum_leverage: Option<Decimal>,
    pub trading: bool,
    pub metadata_version: String,
    pub venue_updated_at: Option<DateTime<Utc>>,
    pub received_at: DateTime<Utc>,
}

/// Connector health and freshness.
#[derive(Clone, Debug)]
pub struct VenueHealth {
    pub venue: Venue,
    pub environment: VenueEnvironment,
    pub status: HealthStatus,
    pub server_time: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub clock_drift_ms: i64,
    pub rate_limit_remaining: Option<u64>,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unavailable,
    FatalConfiguration,
}

#[derive(Clone, Debug)]
pub struct VenueTimestamp {
    pub venue_time: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct MarketSubscription {
    pub instrument: InstrumentId,
    pub depth: u32,
}

#[derive(Clone, Debug)]
pub struct PriceLevel {
    pub price: Decimal,
    pub quantity: Decimal,
}

#[derive(Clone, Debug)]
pub enum MarketEvent {
    BookSnapshot {
        instrument: InstrumentId,
        sequence: u64,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        venue_time: DateTime<Utc>,
        received_at: DateTime<Utc>,
        checksum: Option<String>,
    },
    BookDelta {
        instrument: InstrumentId,
        first_sequence: u64,
        last_sequence: u64,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
        venue_time: DateTime<Utc>,
        received_at: DateTime<Utc>,
        checksum: Option<String>,
    },
    Funding(FundingQuote),
    InstrumentChanged(Instrument),
    Heartbeat(DateTime<Utc>),
}

#[derive(Clone, Debug)]
pub struct FundingQuote {
    pub instrument: InstrumentId,
    pub rate: Decimal,
    pub interval_hours: u32,
    pub mark_price: Decimal,
    pub index_price: Decimal,
    pub next_funding_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct Balance {
    pub asset: String,
    pub total: Decimal,
    pub available: Decimal,
    pub locked: Decimal,
    pub venue_updated_at: Option<DateTime<Utc>>,
    pub received_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct VenuePosition {
    pub instrument: InstrumentId,
    pub quantity: Decimal,
    pub entry_price: Decimal,
    pub mark_price: Decimal,
    pub leverage: Decimal,
    pub liquidation_price: Option<Decimal>,
    pub venue_updated_at: Option<DateTime<Utc>>,
    pub received_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderStatus {
    Pending,
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
    Unknown,
}

impl OrderStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }
}

#[derive(Clone, Debug)]
pub struct CanonicalOrderIntent {
    pub account_id: AccountId,
    pub instrument: InstrumentId,
    pub side: OrderSide,
    pub base_quantity: Decimal,
    pub limit_price: Decimal,
    pub time_in_force: String,
    pub reduce_only: bool,
    pub client_order_id: String,
}

#[derive(Clone, Debug)]
pub struct PreparedOrder {
    pub intent: CanonicalOrderIntent,
    pub native_symbol: String,
    pub native_quantity: Decimal,
    pub equivalent_base_quantity: Decimal,
    pub native_price: Decimal,
    pub projected_delta_usd: Decimal,
    pub payload: serde_json::Value,
    pub payload_hash: String,
}

#[derive(Clone, Debug)]
pub struct OrderAck {
    pub venue_order_id: Option<String>,
    pub client_order_id: String,
    pub status: OrderStatus,
    pub acknowledged_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct CancelRequest {
    pub account_id: AccountId,
    pub instrument: InstrumentId,
    pub venue_order_id: Option<String>,
    pub client_order_id: String,
}

#[derive(Clone, Debug)]
pub struct CancelAck {
    pub status: OrderStatus,
    pub acknowledged_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub enum OrderLookup {
    VenueOrderId(String),
    ClientOrderId(String),
}

#[derive(Clone, Debug)]
pub struct VenueOrder {
    pub instrument: InstrumentId,
    pub venue_order_id: String,
    pub client_order_id: Option<String>,
    pub side: OrderSide,
    pub status: OrderStatus,
    pub requested_quantity: Decimal,
    pub filled_quantity: Decimal,
    pub average_fill_price: Option<Decimal>,
    pub sequence: Option<u64>,
    pub venue_updated_at: Option<DateTime<Utc>>,
    pub received_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct Fill {
    pub venue_fill_id: String,
    pub venue_order_id: String,
    pub instrument: InstrumentId,
    pub quantity: Decimal,
    pub price: Decimal,
    pub fee: Decimal,
    pub fee_asset: String,
    pub occurred_at: DateTime<Utc>,
    pub received_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct FillPage {
    pub fills: Vec<Fill>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FeeSchedule {
    pub product: ProductType,
    pub maker: Decimal,
    pub taker: Decimal,
    pub effective_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct FundingLedgerEntry {
    pub venue_entry_id: String,
    pub instrument: InstrumentId,
    pub amount: Decimal,
    pub asset: String,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub enum PrivateEvent {
    Order(VenueOrder),
    Fill(Fill),
    Balance(Balance),
    Position(VenuePosition),
    Funding(FundingLedgerEntry),
    Heartbeat(DateTime<Utc>),
}

/// Stable error categories used by retry and containment policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectorErrorKind {
    Transient,
    RateLimited,
    UnknownOutcome,
    ReconciliationRequired,
    FatalConfiguration,
    Unsupported,
    InvalidResponse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectorError {
    pub kind: ConnectorErrorKind,
    pub code: String,
    pub message: String,
    pub retry_after_ms: Option<u64>,
}

impl std::fmt::Display for ConnectorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ConnectorError {}

pub type ConnectorResult<T> = Result<T, ConnectorError>;

#[cfg(test)]
mod tests {
    use super::{OrderStatus, VenueEnvironment};

    #[test]
    fn production_is_never_a_sandbox_write_target() {
        assert!(!VenueEnvironment::Production.is_non_production());
        assert!(VenueEnvironment::Testnet.is_non_production());
        assert!(VenueEnvironment::Demo.is_non_production());
    }

    #[test]
    fn only_final_order_states_are_terminal() {
        assert!(OrderStatus::Filled.is_terminal());
        assert!(OrderStatus::Cancelled.is_terminal());
        assert!(OrderStatus::Rejected.is_terminal());
        assert!(!OrderStatus::PartiallyFilled.is_terminal());
        assert!(!OrderStatus::Unknown.is_terminal());
    }
}
