//! Async connector traits. Venue details do not cross this boundary.

use std::pin::Pin;

use async_trait::async_trait;
use funding_arb_domain::market::Venue;
use futures_core::Stream;

use crate::model::{
    AccountId, Balance, CancelAck, CancelRequest, CanonicalOrderIntent, ConnectorResult,
    FeeSchedule, FillPage, FundingLedgerEntry, FundingQuote, Instrument, InstrumentId, MarketEvent,
    MarketSubscription, OrderAck, OrderLookup, PreparedOrder, PrivateEvent, TimeRange,
    VenueCapabilities, VenueHealth, VenueOrder, VenuePosition, VenueTimestamp,
};

pub type MarketEventStream =
    Pin<Box<dyn Stream<Item = ConnectorResult<MarketEvent>> + Send + 'static>>;
pub type PrivateEventStream =
    Pin<Box<dyn Stream<Item = ConnectorResult<PrivateEvent>> + Send + 'static>>;

/// Capability-driven connector used by scanner, execution, and reconciliation.
#[async_trait]
pub trait VenueConnector: Send + Sync {
    fn venue(&self) -> Venue;
    fn capabilities(&self) -> VenueCapabilities;

    async fn health(&self) -> ConnectorResult<VenueHealth>;
    async fn server_time(&self) -> ConnectorResult<VenueTimestamp>;
    async fn instruments(&self) -> ConnectorResult<Vec<Instrument>>;
    async fn market_stream(
        &self,
        subscriptions: Vec<MarketSubscription>,
    ) -> ConnectorResult<MarketEventStream>;
    async fn private_stream(&self, account: &AccountId) -> ConnectorResult<PrivateEventStream>;
    async fn fee_schedule(&self, account: &AccountId) -> ConnectorResult<Vec<FeeSchedule>>;
    async fn current_funding(&self, instrument: &InstrumentId) -> ConnectorResult<FundingQuote>;
    async fn funding_history(
        &self,
        instrument: &InstrumentId,
        range: &TimeRange,
    ) -> ConnectorResult<Vec<FundingQuote>>;
    async fn balances(&self, account: &AccountId) -> ConnectorResult<Vec<Balance>>;
    async fn positions(&self, account: &AccountId) -> ConnectorResult<Vec<VenuePosition>>;
    async fn open_orders(&self, account: &AccountId) -> ConnectorResult<Vec<VenueOrder>>;
    async fn fills(&self, account: &AccountId, cursor: Option<&str>) -> ConnectorResult<FillPage>;
    async fn funding_ledger(
        &self,
        account: &AccountId,
        range: &TimeRange,
    ) -> ConnectorResult<Vec<FundingLedgerEntry>>;
    async fn prepare_order(&self, order: &CanonicalOrderIntent) -> ConnectorResult<PreparedOrder>;
    async fn submit_order(&self, order: &PreparedOrder) -> ConnectorResult<OrderAck>;
    async fn cancel_order(&self, request: &CancelRequest) -> ConnectorResult<CancelAck>;
    async fn query_order(
        &self,
        account: &AccountId,
        key: &OrderLookup,
    ) -> ConnectorResult<Option<VenueOrder>>;
}
