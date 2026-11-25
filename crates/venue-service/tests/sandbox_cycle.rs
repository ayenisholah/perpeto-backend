//! M4 sandbox driver evidence against a live Postgres database and a scripted
//! two-leg venue: a full open → close cycle, a one-legged open that is
//! unwound, a second open refused while one is unfinished, and the forced
//! unknown-outcome recovery.
//!
//! Skipped unless `DATABASE_URL` is set (as in CI). Each test seeds its own
//! tenant. The scripted venue counts every write, so "no duplicate order" is
//! asserted on the venue side.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use funding_arb_connectors::{
    AccountId, Balance, CancelAck, CancelRequest, CanonicalOrderIntent, ConnectorError,
    ConnectorErrorKind, ConnectorResult, FeeSchedule, FillPage, FundingLedgerEntry, FundingQuote,
    Instrument, InstrumentId, MarketEvent, MarketEventStream, MarketSubscription, OrderAck,
    OrderLookup, OrderSide, OrderStatus, PreparedOrder, PriceLevel, PrivateEventStream,
    ProductType, TimeRange, VenueCapabilities, VenueConnector, VenueEnvironment, VenueHealth,
    VenueOrder, VenuePosition, VenueTimestamp,
};
use funding_arb_domain::TradingMode;
use funding_arb_domain::market::Venue;
use funding_arb_tenant_context::{TenantContext, TenantId};
use funding_arb_venue_service::{
    SandboxDriver, SandboxError, SandboxLeg, SandboxVenue, SubmissionState,
};
use rust_decimal::Decimal;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// One product of a scripted venue. IOC orders fill `fill_percent` of their
/// quantity at once; GTC orders rest. `lose_next` makes the next submission
/// reach the book but lose its response.
struct ScriptedLeg {
    product: ProductType,
    fill_percent: u32,
    lose_next: AtomicBool,
    orders: Mutex<HashMap<String, VenueOrder>>,
    submissions: AtomicUsize,
    reduce_only: Mutex<Vec<bool>>,
    base: Mutex<Decimal>,
}

impl ScriptedLeg {
    fn new(product: ProductType, fill_percent: u32) -> Self {
        Self {
            product,
            fill_percent,
            lose_next: AtomicBool::new(false),
            orders: Mutex::new(HashMap::new()),
            submissions: AtomicUsize::new(0),
            reduce_only: Mutex::new(Vec::new()),
            base: Mutex::new(Decimal::ZERO),
        }
    }

    fn id(&self) -> InstrumentId {
        InstrumentId {
            venue: Venue::Bybit,
            symbol: "ETHUSDT".into(),
            product: self.product,
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

fn price(value: i64) -> Decimal {
    Decimal::new(value, 0)
}

#[async_trait]
impl VenueConnector for ScriptedLeg {
    fn venue(&self) -> Venue {
        Venue::Bybit
    }

    fn capabilities(&self) -> VenueCapabilities {
        VenueCapabilities {
            products: vec![self.product],
            market_stream: true,
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
        Ok(vec![Instrument {
            id: self.id(),
            canonical_underlying: "ETH".into(),
            quote_asset: "USDT".into(),
            settlement_asset: "USDT".into(),
            margin_asset: "USDT".into(),
            contract_multiplier: Decimal::ONE,
            price_tick: Decimal::new(1, 2),
            quantity_step: Decimal::new(1, 3),
            minimum_quantity: Decimal::new(1, 3),
            maximum_quantity: None,
            minimum_notional: Decimal::new(5, 0),
            funding_interval_hours: Some(8),
            funding_cap: None,
            funding_floor: None,
            maximum_leverage: None,
            trading: true,
            metadata_version: "scripted".into(),
            venue_updated_at: None,
            received_at: Utc::now(),
        }])
    }

    async fn market_stream(
        &self,
        _: Vec<MarketSubscription>,
    ) -> ConnectorResult<MarketEventStream> {
        let snapshot = MarketEvent::BookSnapshot {
            instrument: self.id(),
            sequence: 1,
            bids: vec![PriceLevel {
                price: price(2999),
                quantity: Decimal::TEN,
            }],
            asks: vec![PriceLevel {
                price: price(3000),
                quantity: Decimal::TEN,
            }],
            venue_time: Utc::now(),
            received_at: Utc::now(),
            checksum: None,
        };
        Ok(Box::pin(futures_util::stream::iter(vec![Ok(snapshot)])))
    }

    async fn private_stream(&self, _: &AccountId) -> ConnectorResult<PrivateEventStream> {
        unsupported()
    }

    async fn fee_schedule(&self, _: &AccountId) -> ConnectorResult<Vec<FeeSchedule>> {
        unsupported()
    }

    async fn current_funding(&self, instrument: &InstrumentId) -> ConnectorResult<FundingQuote> {
        Ok(FundingQuote {
            instrument: instrument.clone(),
            rate: Decimal::new(1, 4),
            interval_hours: 8,
            mark_price: price(3000),
            index_price: price(3000),
            next_funding_at: Utc::now() + chrono::Duration::hours(1),
            observed_at: Utc::now(),
        })
    }

    async fn funding_history(
        &self,
        _: &InstrumentId,
        _: &TimeRange,
    ) -> ConnectorResult<Vec<FundingQuote>> {
        unsupported()
    }

    async fn balances(&self, _: &AccountId) -> ConnectorResult<Vec<Balance>> {
        let base = *self.base.lock().expect("lock");
        Ok(vec![Balance {
            asset: "ETH".into(),
            total: base,
            available: base,
            locked: Decimal::ZERO,
            venue_updated_at: None,
            received_at: Utc::now(),
        }])
    }

    async fn positions(&self, _: &AccountId) -> ConnectorResult<Vec<VenuePosition>> {
        Ok(Vec::new())
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
        self.reduce_only
            .lock()
            .expect("lock")
            .push(order.intent.reduce_only);
        let resting = order.intent.time_in_force == "GTC";
        let filled = if resting {
            Decimal::ZERO
        } else {
            order.intent.base_quantity * Decimal::from(self.fill_percent) / Decimal::ONE_HUNDRED
        };
        let status = if resting {
            OrderStatus::Open
        } else if filled == order.intent.base_quantity {
            OrderStatus::Filled
        } else {
            OrderStatus::Cancelled
        };
        if self.product == ProductType::Spot {
            let mut base = self.base.lock().expect("lock");
            match order.intent.side {
                OrderSide::Buy => *base += filled,
                OrderSide::Sell => *base -= filled,
            }
        }
        let client_order_id = order.intent.client_order_id.clone();
        self.orders.lock().expect("lock").insert(
            client_order_id.clone(),
            VenueOrder {
                instrument: self.id(),
                venue_order_id: format!("v-{client_order_id}"),
                client_order_id: Some(client_order_id.clone()),
                side: order.intent.side,
                status,
                requested_quantity: order.intent.base_quantity,
                filled_quantity: filled,
                average_fill_price: Some(order.intent.limit_price),
                sequence: None,
                venue_updated_at: None,
                received_at: Utc::now(),
            },
        );
        if self.lose_next.swap(false, Ordering::SeqCst) {
            return Err(ConnectorError {
                kind: ConnectorErrorKind::UnknownOutcome,
                code: "BYBIT_NETWORK".into(),
                message: "response lost".into(),
                retry_after_ms: None,
            });
        }
        Ok(OrderAck {
            venue_order_id: Some(format!("v-{client_order_id}")),
            client_order_id,
            status,
            acknowledged_at: Utc::now(),
        })
    }

    async fn cancel_order(&self, request: &CancelRequest) -> ConnectorResult<CancelAck> {
        let mut orders = self.orders.lock().expect("lock");
        let order = orders
            .get_mut(&request.client_order_id)
            .ok_or_else(|| ConnectorError {
                kind: ConnectorErrorKind::InvalidResponse,
                code: "ORDER_NOT_FOUND".into(),
                message: "no such order".into(),
                retry_after_ms: None,
            })?;
        if !order.status.is_terminal() {
            order.status = OrderStatus::Cancelled;
        }
        Ok(CancelAck {
            status: order.status,
            acknowledged_at: Utc::now(),
        })
    }

    async fn query_order(
        &self,
        _: &AccountId,
        key: &OrderLookup,
    ) -> ConnectorResult<Option<VenueOrder>> {
        let OrderLookup::ClientOrderId(client_order_id) = key else {
            return unsupported();
        };
        Ok(self
            .orders
            .lock()
            .expect("lock")
            .get(client_order_id)
            .cloned())
    }
}

async fn setup() -> Option<(TenantContext, SandboxDriver)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect");
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Sandbox driver test')")
        .bind(id)
        .execute(&pool)
        .await
        .expect("tenant");
    let driver = SandboxDriver::new(pool, TradingMode::Sandbox)
        .expect("driver")
        .with_timing(Duration::from_millis(50), Duration::from_millis(10));
    Some((TenantContext::new(TenantId::new(id)), driver))
}

fn venue<'a>(spot: &'a ScriptedLeg, perp: &'a ScriptedLeg) -> SandboxVenue<'a> {
    SandboxVenue {
        environment: VenueEnvironment::Testnet,
        spot: SandboxLeg {
            connector: spot,
            account: AccountId("spot-account".into()),
        },
        perp: SandboxLeg {
            connector: perp,
            account: AccountId("perp-account".into()),
        },
    }
}

#[tokio::test]
async fn a_hedge_opens_on_both_legs_and_closes_flat() {
    let Some((tenant, driver)) = setup().await else {
        return;
    };
    let spot = ScriptedLeg::new(ProductType::Spot, 100);
    let perp = ScriptedLeg::new(ProductType::LinearPerpetual, 100);
    let venue = venue(&spot, &perp);

    let opened = driver
        .open(tenant, &venue, "ETH", Decimal::new(150, 0))
        .await
        .expect("open");
    assert_eq!(opened.state, "OPEN");
    assert_eq!(opened.base_quantity, Decimal::new(5, 2));
    assert_eq!(opened.spot_filled, Decimal::new(5, 2));
    assert_eq!(opened.perp_filled, Decimal::new(5, 2));
    assert!(opened.funding_at.is_some());
    assert!(opened.snapshot_before.is_some());

    assert!(matches!(
        driver
            .open(tenant, &venue, "ETH", Decimal::new(150, 0))
            .await,
        Err(SandboxError::CycleInProgress)
    ));

    let closed = driver.close(tenant, &venue).await.expect("close");
    assert_eq!(closed.state, "CLOSED");
    assert_eq!(closed.spot_closed, Decimal::new(5, 2));
    assert_eq!(closed.perp_closed, Decimal::new(5, 2));
    assert!(closed.closed_at.is_some() && closed.snapshot_after.is_some());
    assert_eq!(spot.submissions.load(Ordering::SeqCst), 2);
    assert_eq!(perp.submissions.load(Ordering::SeqCst), 2);
    assert_eq!(*perp.reduce_only.lock().expect("lock"), vec![false, true]);
    assert_eq!(*spot.base.lock().expect("lock"), Decimal::ZERO);

    assert!(matches!(
        driver.close(tenant, &venue).await,
        Err(SandboxError::NoOpenCycle)
    ));
    let runs = driver
        .runs_since(tenant, "BYBIT", closed.created_at)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
}

#[tokio::test]
async fn a_one_legged_open_is_unwound_and_failed() {
    let Some((tenant, driver)) = setup().await else {
        return;
    };
    let spot = ScriptedLeg::new(ProductType::Spot, 100);
    let perp = ScriptedLeg::new(ProductType::LinearPerpetual, 0);
    let venue = venue(&spot, &perp);

    let error = driver
        .open(tenant, &venue, "ETH", Decimal::new(150, 0))
        .await
        .expect_err("perp leg missed");
    assert!(matches!(error, SandboxError::Market(_)));
    // The spot leg was bought and sold back; the perp missed and is not closed.
    assert_eq!(spot.submissions.load(Ordering::SeqCst), 2);
    assert_eq!(perp.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(*spot.base.lock().expect("lock"), Decimal::ZERO);
    assert!(
        driver
            .active(tenant, "BYBIT")
            .await
            .expect("active")
            .is_none()
    );
}

#[tokio::test]
async fn a_lost_order_is_resolved_before_any_retry_and_cancelled() {
    let Some((tenant, driver)) = setup().await else {
        return;
    };
    let spot = ScriptedLeg::new(ProductType::Spot, 100);
    let perp = ScriptedLeg::new(ProductType::LinearPerpetual, 100);
    perp.lose_next.store(true, Ordering::SeqCst);
    let venue = venue(&spot, &perp);

    let outcome = driver
        .recover(tenant, &venue, "ETH")
        .await
        .expect("recover");
    assert_eq!(outcome.first, SubmissionState::UnknownOutcome);
    assert_eq!(outcome.retry, SubmissionState::Duplicate);
    assert_eq!(outcome.resolved, Some(OrderStatus::Open));
    assert_eq!(outcome.final_status, Some(OrderStatus::Cancelled));
    assert_eq!(outcome.filled, Decimal::ZERO);
    // The retry never reached the venue.
    assert_eq!(perp.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(spot.submissions.load(Ordering::SeqCst), 0);

    let runs = driver
        .runs_since(tenant, "BYBIT", Utc::now() - chrono::Duration::minutes(1))
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].kind, "RECOVERY");
    assert_eq!(runs[0].state, "CLOSED");
    assert_eq!(
        runs[0].outcome.as_ref().expect("outcome")["retry_submission"],
        "Duplicate"
    );
}
