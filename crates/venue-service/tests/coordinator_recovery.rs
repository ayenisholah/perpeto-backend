//! Submit-once and query-before-retry evidence for the sandbox coordinator
//! (M4), against a live Postgres database and a scripted connector.
//!
//! Skipped unless `DATABASE_URL` is set (as in CI). Each test seeds its own
//! tenant. The fake connector counts every venue write, so "no duplicate order"
//! is asserted on the venue side, not only in the database.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use funding_arb_connectors::{
    AccountId, Balance, CancelAck, CancelRequest, CanonicalOrderIntent, ConnectorError,
    ConnectorErrorKind, ConnectorResult, FeeSchedule, FillPage, FundingLedgerEntry, FundingQuote,
    Instrument, InstrumentId, MarketEventStream, MarketSubscription, OrderAck, OrderLookup,
    OrderSide, OrderStatus, PreparedOrder, PrivateEventStream, ProductType, TimeRange,
    VenueCapabilities, VenueConnector, VenueEnvironment, VenueHealth, VenueOrder, VenuePosition,
    VenueTimestamp,
};
use funding_arb_domain::TradingMode;
use funding_arb_domain::market::Venue;
use funding_arb_tenant_context::{TenantContext, TenantId};
use funding_arb_venue_service::{ConnectorExecutionCoordinator, CoordinatorError, SubmissionState};
use rust_decimal::Decimal;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// What the fake venue does with the next submission.
#[derive(Clone, Copy)]
enum Submit {
    Ack,
    LoseResponse,
}

struct ScriptedVenue {
    submit: Submit,
    /// The order the venue reports when queried, if any.
    stored: Mutex<Option<OrderStatus>>,
    submissions: AtomicUsize,
    queries: AtomicUsize,
}

impl ScriptedVenue {
    fn new(submit: Submit, stored: Option<OrderStatus>) -> Self {
        Self {
            submit,
            stored: Mutex::new(stored),
            submissions: AtomicUsize::new(0),
            queries: AtomicUsize::new(0),
        }
    }
}

fn unsupported<T>() -> ConnectorResult<T> {
    Err(ConnectorError {
        kind: ConnectorErrorKind::Unsupported,
        code: "UNSUPPORTED".into(),
        message: "not scripted".into(),
        retry_after_ms: None,
    })
}

fn instrument() -> InstrumentId {
    InstrumentId {
        venue: Venue::Bybit,
        symbol: "ETHUSDT".into(),
        product: ProductType::LinearPerpetual,
    }
}

#[async_trait]
impl VenueConnector for ScriptedVenue {
    fn venue(&self) -> Venue {
        Venue::Bybit
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            products: vec![ProductType::LinearPerpetual],
            market_stream: false,
            private_stream: false,
            funding_history: false,
            order_submission: true,
            borrow: false,
            internal_transfer: false,
        }
    }

    async fn health(&self) -> ConnectorResult<VenueHealth> {
        unsupported()
    }

    async fn server_time(&self) -> ConnectorResult<VenueTimestamp> {
        unsupported()
    }

    async fn instruments(&self) -> ConnectorResult<Vec<Instrument>> {
        unsupported()
    }

    async fn market_stream(
        &self,
        _: Vec<MarketSubscription>,
    ) -> ConnectorResult<MarketEventStream> {
        unsupported()
    }

    async fn private_stream(&self, _: &AccountId) -> ConnectorResult<PrivateEventStream> {
        unsupported()
    }

    async fn fee_schedule(&self, _: &AccountId) -> ConnectorResult<Vec<FeeSchedule>> {
        unsupported()
    }

    async fn current_funding(&self, _: &InstrumentId) -> ConnectorResult<FundingQuote> {
        unsupported()
    }

    async fn funding_history(
        &self,
        _: &InstrumentId,
        _: &TimeRange,
    ) -> ConnectorResult<Vec<FundingQuote>> {
        unsupported()
    }

    async fn balances(&self, _: &AccountId) -> ConnectorResult<Vec<Balance>> {
        unsupported()
    }

    async fn positions(&self, _: &AccountId) -> ConnectorResult<Vec<VenuePosition>> {
        unsupported()
    }

    async fn open_orders(&self, _: &AccountId) -> ConnectorResult<Vec<VenueOrder>> {
        unsupported()
    }

    async fn fills(&self, _: &AccountId, _: Option<&str>) -> ConnectorResult<FillPage> {
        unsupported()
    }

    async fn funding_ledger(
        &self,
        _: &AccountId,
        _: &TimeRange,
    ) -> ConnectorResult<Vec<FundingLedgerEntry>> {
        unsupported()
    }

    async fn prepare_order(&self, order: &CanonicalOrderIntent) -> ConnectorResult<PreparedOrder> {
        Ok(PreparedOrder {
            intent: order.clone(),
            native_symbol: order.instrument.symbol.clone(),
            native_quantity: order.base_quantity,
            equivalent_base_quantity: order.base_quantity,
            native_price: order.limit_price,
            projected_delta_usd: order.base_quantity * order.limit_price,
            payload: serde_json::json!({"orderLinkId": order.client_order_id}),
            payload_hash: format!("hash-{}", order.client_order_id),
        })
    }

    async fn submit_order(&self, order: &PreparedOrder) -> ConnectorResult<OrderAck> {
        self.submissions.fetch_add(1, Ordering::SeqCst);
        match self.submit {
            Submit::Ack => Ok(OrderAck {
                venue_order_id: Some("venue-1".into()),
                client_order_id: order.intent.client_order_id.clone(),
                status: OrderStatus::Filled,
                acknowledged_at: Utc::now(),
            }),
            Submit::LoseResponse => Err(ConnectorError {
                kind: ConnectorErrorKind::UnknownOutcome,
                code: "BYBIT_NETWORK".into(),
                message: "response lost".into(),
                retry_after_ms: None,
            }),
        }
    }

    async fn cancel_order(&self, _: &CancelRequest) -> ConnectorResult<CancelAck> {
        unsupported()
    }

    async fn query_order(
        &self,
        _: &AccountId,
        key: &OrderLookup,
    ) -> ConnectorResult<Option<VenueOrder>> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        let OrderLookup::ClientOrderId(client_order_id) = key else {
            return unsupported();
        };
        let stored = *self.stored.lock().expect("lock");
        Ok(stored.map(|status| VenueOrder {
            instrument: instrument(),
            venue_order_id: "venue-1".into(),
            client_order_id: Some(client_order_id.clone()),
            side: OrderSide::Sell,
            status,
            requested_quantity: Decimal::new(1, 2),
            filled_quantity: if status == OrderStatus::Filled {
                Decimal::new(1, 2)
            } else {
                Decimal::ZERO
            },
            average_fill_price: None,
            sequence: None,
            venue_updated_at: None,
            received_at: Utc::now(),
        }))
    }
}

async fn setup() -> Option<(PgPool, TenantContext, ConnectorExecutionCoordinator)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect");
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Coordinator recovery test')")
        .bind(id)
        .execute(&pool)
        .await
        .expect("tenant");
    let coordinator = ConnectorExecutionCoordinator::new(pool.clone());
    Some((pool, TenantContext::new(TenantId::new(id)), coordinator))
}

fn order(client_order_id: &str) -> CanonicalOrderIntent {
    CanonicalOrderIntent {
        account_id: AccountId("sandbox-account".into()),
        instrument: instrument(),
        side: OrderSide::Sell,
        base_quantity: Decimal::new(1, 2),
        limit_price: Decimal::new(3000, 0),
        time_in_force: "IOC".into(),
        reduce_only: false,
        client_order_id: client_order_id.into(),
    }
}

async fn intent_state(pool: &PgPool, tenant: TenantContext, client_order_id: &str) -> String {
    let mut transaction = funding_arb_tenant_context::TenantTransaction::begin(pool, tenant)
        .await
        .expect("transaction");
    let state: String = sqlx::query_scalar(
        "SELECT state FROM connector_order_intents WHERE tenant_id = $1 AND client_order_id = $2",
    )
    .bind(tenant.tenant_id().as_uuid())
    .bind(client_order_id)
    .fetch_one(transaction.connection())
    .await
    .expect("intent");
    transaction.commit().await.expect("commit");
    state
}

fn client_id() -> String {
    format!("m4t{}", &Uuid::new_v4().simple().to_string()[..20])
}

#[tokio::test]
async fn a_repeated_client_order_id_never_reaches_the_venue_twice() {
    let Some((pool, tenant, coordinator)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let venue = ScriptedVenue::new(Submit::Ack, None);
    let id = client_id();
    let first = coordinator
        .submit_sandbox(
            tenant,
            TradingMode::Sandbox,
            VenueEnvironment::Testnet,
            &venue,
            &order(&id),
        )
        .await
        .expect("first submit");
    assert_eq!(first.state, SubmissionState::Acknowledged);
    let second = coordinator
        .submit_sandbox(
            tenant,
            TradingMode::Sandbox,
            VenueEnvironment::Testnet,
            &venue,
            &order(&id),
        )
        .await
        .expect("second submit");
    assert_eq!(second.state, SubmissionState::Duplicate);
    assert_eq!(second.intent_id, first.intent_id);
    assert_eq!(venue.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(intent_state(&pool, tenant, &id).await, "FILLED");
}

#[tokio::test]
async fn a_lost_response_is_queried_before_anything_is_resent() {
    let Some((pool, tenant, coordinator)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    // The venue executed the order, but its response never arrived.
    let venue = ScriptedVenue::new(Submit::LoseResponse, Some(OrderStatus::Filled));
    let id = client_id();
    let submitted = coordinator
        .submit_sandbox(
            tenant,
            TradingMode::Sandbox,
            VenueEnvironment::Testnet,
            &venue,
            &order(&id),
        )
        .await
        .expect("submit");
    assert_eq!(submitted.state, SubmissionState::UnknownOutcome);
    assert_eq!(intent_state(&pool, tenant, &id).await, "UNKNOWN_OUTCOME");
    let unresolved = coordinator.unresolved(tenant).await.expect("unresolved");
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].client_order_id, id);

    // A retry with the same client order id is answered locally.
    let retry = coordinator
        .submit_sandbox(
            tenant,
            TradingMode::Sandbox,
            VenueEnvironment::Testnet,
            &venue,
            &order(&id),
        )
        .await
        .expect("retry");
    assert_eq!(retry.state, SubmissionState::Duplicate);
    assert_eq!(venue.submissions.load(Ordering::SeqCst), 1);

    let found = coordinator
        .reconcile_unknown(tenant, &venue, &AccountId("sandbox-account".into()), &id)
        .await
        .expect("reconcile");
    assert_eq!(found.map(|order| order.status), Some(OrderStatus::Filled));
    assert_eq!(venue.queries.load(Ordering::SeqCst), 1);
    assert_eq!(intent_state(&pool, tenant, &id).await, "FILLED");
    assert!(
        coordinator
            .unresolved(tenant)
            .await
            .expect("unresolved")
            .is_empty()
    );
}

#[tokio::test]
async fn a_lost_order_the_venue_never_saw_becomes_not_found() {
    let Some((pool, tenant, coordinator)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let venue = ScriptedVenue::new(Submit::LoseResponse, None);
    let id = client_id();
    coordinator
        .submit_sandbox(
            tenant,
            TradingMode::Sandbox,
            VenueEnvironment::Testnet,
            &venue,
            &order(&id),
        )
        .await
        .expect("submit");
    let found = coordinator
        .reconcile_unknown(tenant, &venue, &AccountId("sandbox-account".into()), &id)
        .await
        .expect("reconcile");
    assert!(found.is_none());
    assert_eq!(intent_state(&pool, tenant, &id).await, "NOT_FOUND");
    assert!(
        coordinator
            .unresolved(tenant)
            .await
            .expect("unresolved")
            .is_empty()
    );
}

#[tokio::test]
async fn paper_mode_and_production_never_write() {
    let Some((_pool, tenant, coordinator)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let venue = ScriptedVenue::new(Submit::Ack, None);
    let paper = coordinator
        .submit_sandbox(
            tenant,
            TradingMode::Paper,
            VenueEnvironment::Testnet,
            &venue,
            &order(&client_id()),
        )
        .await;
    assert!(matches!(paper, Err(CoordinatorError::ModeForbidden)));
    let production = coordinator
        .submit_sandbox(
            tenant,
            TradingMode::Sandbox,
            VenueEnvironment::Production,
            &venue,
            &order(&client_id()),
        )
        .await;
    assert!(matches!(
        production,
        Err(CoordinatorError::ProductionWriteForbidden)
    ));
    assert_eq!(venue.submissions.load(Ordering::SeqCst), 0);
}
