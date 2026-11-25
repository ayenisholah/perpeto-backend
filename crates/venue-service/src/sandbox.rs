//! M4 sandbox driver: hedged spot + perpetual cycles and a forced
//! unknown-outcome recovery on one testnet or demo venue.
//!
//! Every order goes through [`ConnectorExecutionCoordinator::submit_sandbox`],
//! so it is persisted before it is sent, never sent twice for one client order
//! id, and refused outside SANDBOX mode or against a production endpoint. The
//! driver adds the two-leg bookkeeping in `sandbox_cycles`: client order ids are
//! stored before their orders are submitted, so a restarted `close` reuses them
//! and the coordinator answers `Duplicate` instead of sending a second order.

use std::fmt;
use std::time::Duration;

use chrono::{DateTime, Utc};
use funding_arb_connectors::{
    AccountId, CancelRequest, CanonicalOrderIntent, ConnectorError, Instrument, MarketEvent,
    MarketSubscription, OrderLookup, OrderSide, OrderStatus, ProductType, VenueConnector,
    VenueEnvironment, VenueOrder,
};
use funding_arb_domain::TradingMode;
use funding_arb_tenant_context::{TenantContext, TenantTransaction};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::coordinator::{ConnectorExecutionCoordinator, CoordinatorError, SubmissionState};

/// One leg of a venue: the connector built for one enrolled product.
pub struct SandboxLeg<'a> {
    pub connector: &'a dyn VenueConnector,
    pub account: AccountId,
}

/// The spot and perpetual legs of one venue, both in the same environment.
pub struct SandboxVenue<'a> {
    pub environment: VenueEnvironment,
    pub spot: SandboxLeg<'a>,
    pub perp: SandboxLeg<'a>,
}

impl SandboxVenue<'_> {
    fn name(&self) -> &'static str {
        self.perp.connector.venue().as_str()
    }
}

#[derive(Debug)]
pub enum SandboxError {
    ModeForbidden,
    ProductionForbidden,
    Instrument(String),
    Market(String),
    BelowMinimum { required_notional: Decimal },
    CycleInProgress,
    NoOpenCycle,
    Coordinator(CoordinatorError),
    Connector(ConnectorError),
    Persistence,
}

impl fmt::Display for SandboxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModeForbidden => {
                formatter.write_str("the sandbox driver runs only in SANDBOX mode")
            }
            Self::ProductionForbidden => {
                formatter.write_str("the sandbox driver refuses production connections")
            }
            Self::Instrument(message) | Self::Market(message) => formatter.write_str(message),
            Self::BelowMinimum { required_notional } => write!(
                formatter,
                "notional is below the venue minimums; use at least {required_notional} USDT"
            ),
            Self::CycleInProgress => {
                formatter.write_str("an unfinished cycle exists on this venue; close it first")
            }
            Self::NoOpenCycle => formatter.write_str("no unfinished cycle exists on this venue"),
            Self::Coordinator(CoordinatorError::Connector(error)) | Self::Connector(error) => {
                write!(formatter, "{}: {}", error.code, error.message)
            }
            Self::Coordinator(error) => write!(formatter, "{error}"),
            Self::Persistence => formatter.write_str("sandbox cycle persistence failed"),
        }
    }
}

impl std::error::Error for SandboxError {}

/// A persisted sandbox run.
#[derive(Clone, Debug)]
pub struct SandboxCycle {
    pub id: Uuid,
    pub kind: String,
    pub venue: String,
    pub underlying: String,
    pub spot_symbol: Option<String>,
    pub perp_symbol: String,
    pub base_quantity: Decimal,
    pub spot_open_client_id: Option<String>,
    pub perp_open_client_id: String,
    pub spot_close_client_id: Option<String>,
    pub perp_close_client_id: Option<String>,
    pub spot_filled: Decimal,
    pub perp_filled: Decimal,
    pub spot_closed: Decimal,
    pub perp_closed: Decimal,
    pub state: String,
    pub funding_at: Option<DateTime<Utc>>,
    pub snapshot_before: Option<Value>,
    pub snapshot_after: Option<Value>,
    pub outcome: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub opened_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// The quantity and marketable limit prices of a new hedge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HedgePlan {
    pub quantity: Decimal,
    pub spot_price: Decimal,
    pub perp_price: Decimal,
}

/// What the forced recovery observed, step by step.
#[derive(Clone, Debug)]
pub struct RecoveryOutcome {
    pub cycle_id: Uuid,
    pub client_order_id: String,
    pub first: SubmissionState,
    pub retry: SubmissionState,
    pub resolved: Option<OrderStatus>,
    pub final_status: Option<OrderStatus>,
    pub filled: Decimal,
}

/// Drives M4 runs through the coordinator.
pub struct SandboxDriver {
    pool: PgPool,
    coordinator: ConnectorExecutionCoordinator,
    mode: TradingMode,
    slippage: Decimal,
    fill_wait: Duration,
    poll_interval: Duration,
    book_timeout: Duration,
}

/// Marketable limits cross the touch by this fraction (1%).
const DEFAULT_SLIPPAGE: Decimal = Decimal::from_parts(1, 0, 0, false, 2);
/// The recovery order rests this far above the best ask (2%), so it never fills.
const RESTING_OFFSET: Decimal = Decimal::from_parts(2, 0, 0, false, 2);

impl SandboxDriver {
    /// # Errors
    /// [`SandboxError::ModeForbidden`] unless `mode` is SANDBOX.
    pub fn new(pool: PgPool, mode: TradingMode) -> Result<Self, SandboxError> {
        if mode != TradingMode::Sandbox {
            return Err(SandboxError::ModeForbidden);
        }
        Ok(Self {
            coordinator: ConnectorExecutionCoordinator::new(pool.clone()),
            pool,
            mode,
            slippage: DEFAULT_SLIPPAGE,
            fill_wait: Duration::from_secs(10),
            poll_interval: Duration::from_millis(500),
            book_timeout: Duration::from_secs(15),
        })
    }

    /// Shorter waits, for tests against a scripted venue.
    #[must_use]
    pub fn with_timing(mut self, fill_wait: Duration, poll_interval: Duration) -> Self {
        self.fill_wait = fill_wait;
        self.poll_interval = poll_interval;
        self
    }

    #[must_use]
    pub fn with_slippage(mut self, slippage: Decimal) -> Self {
        self.slippage = slippage;
        self
    }

    fn guard(&self, venue: &SandboxVenue<'_>) -> Result<(), SandboxError> {
        if self.mode != TradingMode::Sandbox {
            return Err(SandboxError::ModeForbidden);
        }
        if !venue.environment.is_non_production() {
            return Err(SandboxError::ProductionForbidden);
        }
        Ok(())
    }

    /// Open a hedge: spot BUY and perpetual SELL of the same base quantity.
    ///
    /// If either leg does not fill, the other is unwound and the cycle is
    /// recorded as FAILED.
    ///
    /// # Errors
    /// Returns [`SandboxError`] for a forbidden mode or environment, an
    /// unfinished cycle, unusable instruments or books, or a venue failure.
    pub async fn open(
        &self,
        context: TenantContext,
        venue: &SandboxVenue<'_>,
        underlying: &str,
        notional: Decimal,
    ) -> Result<SandboxCycle, SandboxError> {
        self.guard(venue)?;
        if self.active(context, venue.name()).await?.is_some() {
            return Err(SandboxError::CycleInProgress);
        }
        let spot = find_instrument(venue.spot.connector, underlying, ProductType::Spot).await?;
        let perp = find_instrument(
            venue.perp.connector,
            underlying,
            ProductType::LinearPerpetual,
        )
        .await?;
        let (_, spot_ask) = top_of_book(venue.spot.connector, &spot, self.book_timeout).await?;
        let (perp_bid, _) = top_of_book(venue.perp.connector, &perp, self.book_timeout).await?;
        let plan = size_hedge(&spot, &perp, spot_ask, perp_bid, notional, self.slippage)?;
        let before = snapshot(venue).await;
        let cycle_id = Uuid::new_v4();
        let spot_id = client_id("so");
        let perp_id = client_id("po");
        self.insert(
            context,
            &NewRun {
                id: cycle_id,
                kind: "CYCLE",
                venue,
                underlying,
                spot_symbol: Some(&spot.id.symbol),
                perp_symbol: &perp.id.symbol,
                quantity: plan.quantity,
                spot_client_id: Some(&spot_id),
                perp_client_id: &perp_id,
                snapshot: &before,
            },
        )
        .await?;

        let spot_order = self
            .place(
                context,
                venue.environment,
                &venue.spot,
                intent(
                    &venue.spot,
                    &spot,
                    OrderSide::Buy,
                    plan.quantity,
                    plan.spot_price,
                    false,
                    &spot_id,
                ),
            )
            .await;
        let spot_filled = match &spot_order {
            Ok(order) => filled(order.as_ref()),
            Err(_) => Decimal::ZERO,
        };
        let hedge_quantity = floor_to(spot_filled.min(plan.quantity), base_step(&perp));
        let perp_order = if hedge_quantity > Decimal::ZERO {
            Some(
                self.place(
                    context,
                    venue.environment,
                    &venue.perp,
                    intent(
                        &venue.perp,
                        &perp,
                        OrderSide::Sell,
                        hedge_quantity,
                        plan.perp_price,
                        false,
                        &perp_id,
                    ),
                )
                .await,
            )
        } else {
            None
        };
        let perp_filled = match &perp_order {
            Some(Ok(order)) => filled(order.as_ref()),
            _ => Decimal::ZERO,
        };
        self.record_open(context, cycle_id, spot_filled, perp_filled)
            .await?;

        if spot_filled > Decimal::ZERO && perp_filled > Decimal::ZERO {
            let funding_at = venue
                .perp
                .connector
                .current_funding(&perp.id)
                .await
                .ok()
                .map(|quote| quote.next_funding_at);
            self.set_state(context, cycle_id, "OPEN", funding_at, None, None)
                .await?;
        } else {
            // One leg missed: unwind whatever filled, then fail the cycle.
            let cycle = self.load(context, cycle_id).await?;
            let unwound = self
                .close_legs(context, venue, &cycle, &spot, &perp, "FAILED")
                .await;
            spot_order?;
            if let Some(order) = perp_order {
                order?;
            }
            unwound?;
            return Err(SandboxError::Market(format!(
                "the hedge did not fill on both legs (spot {spot_filled}, perp {perp_filled}); \
                 the filled leg was unwound"
            )));
        }
        self.load(context, cycle_id).await
    }

    /// Close the venue's unfinished cycle: perpetual BUY `reduce_only` and spot
    /// SELL of what the account holds, up to the opened quantity.
    ///
    /// # Errors
    /// Returns [`SandboxError`] when there is no unfinished cycle or a venue or
    /// persistence step fails. The cycle then stays CLOSING and a rerun resumes
    /// it with the same client order ids.
    pub async fn close(
        &self,
        context: TenantContext,
        venue: &SandboxVenue<'_>,
    ) -> Result<SandboxCycle, SandboxError> {
        self.guard(venue)?;
        let mut cycle = self
            .active(context, venue.name())
            .await?
            .ok_or(SandboxError::NoOpenCycle)?;
        let spot =
            find_instrument(venue.spot.connector, &cycle.underlying, ProductType::Spot).await?;
        let perp = find_instrument(
            venue.perp.connector,
            &cycle.underlying,
            ProductType::LinearPerpetual,
        )
        .await?;
        if cycle.state == "OPENING" {
            // The open was interrupted: learn what filled before closing.
            let spot_filled = match &cycle.spot_open_client_id {
                Some(id) => filled(self.settle(&venue.spot, &spot, id).await?.as_ref()),
                None => Decimal::ZERO,
            };
            let perp_filled = filled(
                self.settle(&venue.perp, &perp, &cycle.perp_open_client_id)
                    .await?
                    .as_ref(),
            );
            self.record_open(context, cycle.id, spot_filled, perp_filled)
                .await?;
            cycle = self.load(context, cycle.id).await?;
        }
        self.close_legs(context, venue, &cycle, &spot, &perp, "CLOSED")
            .await?;
        self.load(context, cycle.id).await
    }

    async fn close_legs(
        &self,
        context: TenantContext,
        venue: &SandboxVenue<'_>,
        cycle: &SandboxCycle,
        spot: &Instrument,
        perp: &Instrument,
        final_state: &str,
    ) -> Result<(), SandboxError> {
        let spot_close_id = cycle
            .spot_close_client_id
            .clone()
            .unwrap_or_else(|| client_id("sc"));
        let perp_close_id = cycle
            .perp_close_client_id
            .clone()
            .unwrap_or_else(|| client_id("pc"));
        self.begin_close(context, cycle.id, &spot_close_id, &perp_close_id)
            .await?;

        let mut perp_closed = Decimal::ZERO;
        if cycle.perp_filled > Decimal::ZERO {
            let (_, ask) = top_of_book(venue.perp.connector, perp, self.book_timeout).await?;
            let price = ceil_to(ask * (Decimal::ONE + self.slippage), perp.price_tick);
            let order = self
                .place(
                    context,
                    venue.environment,
                    &venue.perp,
                    intent(
                        &venue.perp,
                        perp,
                        OrderSide::Buy,
                        cycle.perp_filled,
                        price,
                        true,
                        &perp_close_id,
                    ),
                )
                .await?;
            perp_closed = filled(order.as_ref());
        }

        let mut spot_closed = Decimal::ZERO;
        if cycle.spot_filled > Decimal::ZERO {
            let held = base_available(venue, &spot.canonical_underlying).await;
            let quantity = floor_to(cycle.spot_filled.min(held), base_step(spot));
            let (bid, _) = top_of_book(venue.spot.connector, spot, self.book_timeout).await?;
            let price = floor_to(bid * (Decimal::ONE - self.slippage), spot.price_tick);
            // A spot fee charged in the base asset leaves dust below the
            // venue minimum; that dust stays and the report shows it.
            if quantity >= base_minimum(spot)
                && quantity * price >= spot.minimum_notional
                && quantity > Decimal::ZERO
            {
                let order = self
                    .place(
                        context,
                        venue.environment,
                        &venue.spot,
                        intent(
                            &venue.spot,
                            spot,
                            OrderSide::Sell,
                            quantity,
                            price,
                            false,
                            &spot_close_id,
                        ),
                    )
                    .await?;
                spot_closed = filled(order.as_ref());
            }
        }
        let after = snapshot(venue).await;
        self.record_close(
            context,
            cycle.id,
            spot_closed,
            perp_closed,
            &after,
            final_state,
        )
        .await
    }

    /// Force one unknown outcome and resolve it by query before retry.
    ///
    /// The order is a perpetual SELL resting 2% above the best ask, sized to
    /// the venue minimum, so it should not fill. It is submitted with the
    /// submit fault timeout (`PERPETO_FAULT_SUBMIT_TIMEOUT_MS`) so that its
    /// response is lost; the same client order id is then retried, which the
    /// coordinator must refuse without contacting the venue; then the venue is
    /// queried, the order cancelled, and anything that filled is flattened.
    ///
    /// # Errors
    /// Returns [`SandboxError`] for a forbidden mode or environment, unusable
    /// instruments or books, or a venue or persistence failure.
    pub async fn recover(
        &self,
        context: TenantContext,
        venue: &SandboxVenue<'_>,
        underlying: &str,
    ) -> Result<RecoveryOutcome, SandboxError> {
        self.guard(venue)?;
        // Loading instruments first also warms the connection, so the faulted
        // POST leaves this host before its short timeout expires.
        let perp = find_instrument(
            venue.perp.connector,
            underlying,
            ProductType::LinearPerpetual,
        )
        .await?;
        let (_, ask) = top_of_book(venue.perp.connector, &perp, self.book_timeout).await?;
        let price = ceil_to(ask * (Decimal::ONE + RESTING_OFFSET), perp.price_tick);
        let quantity = minimum_quantity(&perp, price);
        let cycle_id = Uuid::new_v4();
        let order_id = client_id("rf");
        let before = snapshot(venue).await;
        self.insert(
            context,
            &NewRun {
                id: cycle_id,
                kind: "RECOVERY",
                venue,
                underlying,
                spot_symbol: None,
                perp_symbol: &perp.id.symbol,
                quantity,
                spot_client_id: None,
                perp_client_id: &order_id,
                snapshot: &before,
            },
        )
        .await?;
        let order = intent(
            &venue.perp,
            &perp,
            OrderSide::Sell,
            quantity,
            price,
            false,
            &order_id,
        );
        let order = CanonicalOrderIntent {
            time_in_force: "GTC".into(),
            ..order
        };
        let first = self
            .coordinator
            .submit_sandbox(
                context,
                self.mode,
                venue.environment,
                venue.perp.connector,
                &order,
            )
            .await
            .map_err(SandboxError::Coordinator)?;
        let retry = self
            .coordinator
            .submit_sandbox(
                context,
                self.mode,
                venue.environment,
                venue.perp.connector,
                &order,
            )
            .await
            .map_err(SandboxError::Coordinator)?;
        let resolved = if first.state == SubmissionState::UnknownOutcome {
            self.coordinator
                .reconcile_unknown(
                    context,
                    venue.perp.connector,
                    &venue.perp.account,
                    &order_id,
                )
                .await
                .map_err(SandboxError::Coordinator)?
        } else {
            query(&venue.perp, &order_id).await?
        };
        let mut last = resolved.clone();
        if let Some(open) = resolved
            .as_ref()
            .filter(|order| !order.status.is_terminal())
        {
            let _ = venue
                .perp
                .connector
                .cancel_order(&CancelRequest {
                    account_id: venue.perp.account.clone(),
                    instrument: perp.id.clone(),
                    venue_order_id: Some(open.venue_order_id.clone()),
                    client_order_id: order_id.clone(),
                })
                .await;
            last = self.settle(&venue.perp, &perp, &order_id).await?;
        }
        let filled_quantity = filled(last.as_ref());
        let mut flattened = Decimal::ZERO;
        let flatten_id = client_id("rc");
        if filled_quantity > Decimal::ZERO {
            let (_, ask) = top_of_book(venue.perp.connector, &perp, self.book_timeout).await?;
            let price = ceil_to(ask * (Decimal::ONE + self.slippage), perp.price_tick);
            let order = self
                .place(
                    context,
                    venue.environment,
                    &venue.perp,
                    intent(
                        &venue.perp,
                        &perp,
                        OrderSide::Buy,
                        filled_quantity,
                        price,
                        true,
                        &flatten_id,
                    ),
                )
                .await?;
            flattened = filled(order.as_ref());
        }
        let outcome = RecoveryOutcome {
            cycle_id,
            client_order_id: order_id,
            first: first.state,
            retry: retry.state,
            resolved: resolved.as_ref().map(|order| order.status),
            final_status: last.as_ref().map(|order| order.status),
            filled: filled_quantity,
        };
        let after = snapshot(venue).await;
        self.record_recovery(context, &outcome, flattened, &flatten_id, &after)
            .await?;
        Ok(outcome)
    }

    /// Submit through the coordinator, resolve any ambiguity, and wait for the
    /// order to finish. An order still working after the wait is cancelled.
    async fn place(
        &self,
        context: TenantContext,
        environment: VenueEnvironment,
        leg: &SandboxLeg<'_>,
        order: CanonicalOrderIntent,
    ) -> Result<Option<VenueOrder>, SandboxError> {
        let submitted = self
            .coordinator
            .submit_sandbox(context, self.mode, environment, leg.connector, &order)
            .await
            .map_err(SandboxError::Coordinator)?;
        if matches!(
            submitted.state,
            SubmissionState::UnknownOutcome | SubmissionState::Duplicate
        ) {
            // Query before anything else: a resumed run lands here too, and
            // learns the venue state of the order it already sent.
            self.coordinator
                .reconcile_unknown(context, leg.connector, &leg.account, &order.client_order_id)
                .await
                .map_err(SandboxError::Coordinator)?;
        }
        let instrument_id = order.instrument.clone();
        let deadline = tokio::time::Instant::now() + self.fill_wait;
        loop {
            let current = query(leg, &order.client_order_id).await;
            let expired = tokio::time::Instant::now() >= deadline;
            match current {
                Ok(Some(found)) if found.status.is_terminal() => return Ok(Some(found)),
                Ok(Some(found)) if expired => {
                    let _ = leg
                        .connector
                        .cancel_order(&CancelRequest {
                            account_id: leg.account.clone(),
                            instrument: instrument_id,
                            venue_order_id: Some(found.venue_order_id.clone()),
                            client_order_id: order.client_order_id.clone(),
                        })
                        .await;
                    return Ok(query(leg, &order.client_order_id).await?.or(Some(found)));
                }
                Ok(None) if expired => return Ok(None),
                Err(error) if expired => return Err(error),
                _ => tokio::time::sleep(self.poll_interval).await,
            }
        }
    }

    /// Wait for an already-submitted order to reach a terminal state.
    async fn settle(
        &self,
        leg: &SandboxLeg<'_>,
        instrument: &Instrument,
        client_order_id: &str,
    ) -> Result<Option<VenueOrder>, SandboxError> {
        let deadline = tokio::time::Instant::now() + self.fill_wait;
        loop {
            let current = query(leg, client_order_id).await?;
            let done = current
                .as_ref()
                .is_none_or(|order| order.status.is_terminal());
            if done || tokio::time::Instant::now() >= deadline {
                if let Some(open) = current.as_ref().filter(|order| !order.status.is_terminal()) {
                    let _ = leg
                        .connector
                        .cancel_order(&CancelRequest {
                            account_id: leg.account.clone(),
                            instrument: instrument.id.clone(),
                            venue_order_id: Some(open.venue_order_id.clone()),
                            client_order_id: client_order_id.into(),
                        })
                        .await;
                    return Ok(query(leg, client_order_id).await?.or(current));
                }
                return Ok(current);
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    /// The venue's unfinished CYCLE, if any.
    ///
    /// # Errors
    /// [`SandboxError::Persistence`] when the read fails.
    pub async fn active(
        &self,
        context: TenantContext,
        venue: &str,
    ) -> Result<Option<SandboxCycle>, SandboxError> {
        let sql = format!(
            "{SELECT_CYCLE} WHERE tenant_id = $1 AND venue = $2 AND kind = 'CYCLE'
               AND state IN ('OPENING', 'OPEN', 'CLOSING')"
        );
        self.fetch(context, &sql, |query| query.bind(venue))
            .await
            .map(|mut rows| rows.pop())
    }

    /// Every run on `venue` created at or after `since`, oldest first.
    ///
    /// # Errors
    /// [`SandboxError::Persistence`] when the read fails.
    pub async fn runs_since(
        &self,
        context: TenantContext,
        venue: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<SandboxCycle>, SandboxError> {
        let sql = format!(
            "{SELECT_CYCLE} WHERE tenant_id = $1 AND venue = $2 AND created_at >= $3
             ORDER BY created_at"
        );
        self.fetch(context, &sql, |query| query.bind(venue).bind(since))
            .await
    }

    async fn load(&self, context: TenantContext, id: Uuid) -> Result<SandboxCycle, SandboxError> {
        let sql = format!("{SELECT_CYCLE} WHERE tenant_id = $1 AND id = $2");
        self.fetch(context, &sql, |query| query.bind(id))
            .await?
            .pop()
            .ok_or(SandboxError::Persistence)
    }

    async fn fetch<'q>(
        &self,
        context: TenantContext,
        sql: &'q str,
        bind: impl FnOnce(CycleQuery<'q>) -> CycleQuery<'q>,
    ) -> Result<Vec<SandboxCycle>, SandboxError> {
        let mut transaction = TenantTransaction::begin(&self.pool, context)
            .await
            .map_err(|_| SandboxError::Persistence)?;
        let query = bind(sqlx::query_as::<_, CycleRow>(sql).bind(context.tenant_id().as_uuid()));
        let rows = query
            .fetch_all(transaction.connection())
            .await
            .map_err(|_| SandboxError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| SandboxError::Persistence)?;
        rows.into_iter().map(SandboxCycle::try_from).collect()
    }

    async fn execute(
        &self,
        context: TenantContext,
        sql: &str,
        bind: impl for<'q> FnOnce(
            sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
        )
            -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    ) -> Result<(), SandboxError> {
        let mut transaction = TenantTransaction::begin(&self.pool, context)
            .await
            .map_err(|_| SandboxError::Persistence)?;
        bind(sqlx::query(sql).bind(context.tenant_id().as_uuid()))
            .execute(transaction.connection())
            .await
            .map_err(|_| SandboxError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| SandboxError::Persistence)
    }

    async fn insert(&self, context: TenantContext, run: &NewRun<'_>) -> Result<(), SandboxError> {
        let environment = run.venue.environment.as_str();
        self.execute(
            context,
            "INSERT INTO sandbox_cycles
             (tenant_id, id, kind, venue, environment, underlying, spot_symbol, perp_symbol,
              spot_account_id, perp_account_id, base_quantity, spot_open_client_id,
              perp_open_client_id, state, snapshot_before)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::numeric, $12, $13,
                     'OPENING', $14)",
            |query| {
                query
                    .bind(run.id)
                    .bind(run.kind)
                    .bind(run.venue.name())
                    .bind(environment)
                    .bind(run.underlying.to_owned())
                    .bind(run.spot_symbol.map(str::to_owned))
                    .bind(run.perp_symbol.to_owned())
                    .bind(run.spot_client_id.map(|_| run.venue.spot.account.0.clone()))
                    .bind(run.venue.perp.account.0.clone())
                    .bind(run.quantity.to_string())
                    .bind(run.spot_client_id.map(str::to_owned))
                    .bind(run.perp_client_id.to_owned())
                    .bind(run.snapshot.clone())
            },
        )
        .await
    }

    async fn record_open(
        &self,
        context: TenantContext,
        id: Uuid,
        spot_filled: Decimal,
        perp_filled: Decimal,
    ) -> Result<(), SandboxError> {
        self.execute(
            context,
            "UPDATE sandbox_cycles SET spot_filled = $3::numeric, perp_filled = $4::numeric,
                    opened_at = coalesce(opened_at, now()), updated_at = now()
             WHERE tenant_id = $1 AND id = $2",
            |query| {
                query
                    .bind(id)
                    .bind(spot_filled.to_string())
                    .bind(perp_filled.to_string())
            },
        )
        .await
    }

    async fn set_state(
        &self,
        context: TenantContext,
        id: Uuid,
        state: &str,
        funding_at: Option<DateTime<Utc>>,
        outcome: Option<Value>,
        after: Option<Value>,
    ) -> Result<(), SandboxError> {
        let state = state.to_owned();
        self.execute(
            context,
            "UPDATE sandbox_cycles SET state = $3, funding_at = coalesce($4, funding_at),
                    outcome = coalesce($5, outcome), snapshot_after = coalesce($6, snapshot_after),
                    closed_at = CASE WHEN $3 IN ('CLOSED', 'FAILED') THEN now() ELSE closed_at END,
                    updated_at = now()
             WHERE tenant_id = $1 AND id = $2",
            |query| {
                query
                    .bind(id)
                    .bind(state)
                    .bind(funding_at)
                    .bind(outcome)
                    .bind(after)
            },
        )
        .await
    }

    async fn begin_close(
        &self,
        context: TenantContext,
        id: Uuid,
        spot_close_id: &str,
        perp_close_id: &str,
    ) -> Result<(), SandboxError> {
        let (spot_close_id, perp_close_id) = (spot_close_id.to_owned(), perp_close_id.to_owned());
        self.execute(
            context,
            "UPDATE sandbox_cycles
             SET spot_close_client_id = coalesce(spot_close_client_id, $3),
                 perp_close_client_id = coalesce(perp_close_client_id, $4),
                 state = 'CLOSING', updated_at = now()
             WHERE tenant_id = $1 AND id = $2",
            |query| query.bind(id).bind(spot_close_id).bind(perp_close_id),
        )
        .await
    }

    async fn record_close(
        &self,
        context: TenantContext,
        id: Uuid,
        spot_closed: Decimal,
        perp_closed: Decimal,
        after: &Value,
        state: &str,
    ) -> Result<(), SandboxError> {
        self.execute(
            context,
            "UPDATE sandbox_cycles SET spot_closed = $3::numeric, perp_closed = $4::numeric
             WHERE tenant_id = $1 AND id = $2",
            |query| {
                query
                    .bind(id)
                    .bind(spot_closed.to_string())
                    .bind(perp_closed.to_string())
            },
        )
        .await?;
        self.set_state(context, id, state, None, None, Some(after.clone()))
            .await
    }

    async fn record_recovery(
        &self,
        context: TenantContext,
        outcome: &RecoveryOutcome,
        flattened: Decimal,
        flatten_id: &str,
        after: &Value,
    ) -> Result<(), SandboxError> {
        let flatten_id = (outcome.filled > Decimal::ZERO).then(|| flatten_id.to_owned());
        self.execute(
            context,
            "UPDATE sandbox_cycles SET perp_filled = $3::numeric, perp_closed = $4::numeric,
                    perp_close_client_id = $5, opened_at = now()
             WHERE tenant_id = $1 AND id = $2",
            |query| {
                query
                    .bind(outcome.cycle_id)
                    .bind(outcome.filled.to_string())
                    .bind(flattened.to_string())
                    .bind(flatten_id)
            },
        )
        .await?;
        let summary = json!({
            "client_order_id": outcome.client_order_id,
            "first_submission": format!("{:?}", outcome.first),
            "retry_submission": format!("{:?}", outcome.retry),
            "resolved_venue_status": outcome.resolved.map(|status| format!("{status:?}")),
            "final_venue_status": outcome.final_status.map(|status| format!("{status:?}")),
            "filled": outcome.filled.to_string(),
            "flattened": flattened.to_string(),
        });
        self.set_state(
            context,
            outcome.cycle_id,
            "CLOSED",
            None,
            Some(summary),
            Some(after.clone()),
        )
        .await
    }
}

struct NewRun<'a> {
    id: Uuid,
    kind: &'static str,
    venue: &'a SandboxVenue<'a>,
    underlying: &'a str,
    spot_symbol: Option<&'a str>,
    perp_symbol: &'a str,
    quantity: Decimal,
    spot_client_id: Option<&'a str>,
    perp_client_id: &'a str,
    snapshot: &'a Value,
}

type CycleQuery<'q> =
    sqlx::query::QueryAs<'q, sqlx::Postgres, CycleRow, sqlx::postgres::PgArguments>;

const SELECT_CYCLE: &str = "SELECT id, kind, venue, underlying, spot_symbol, perp_symbol,
        base_quantity::text, spot_open_client_id, perp_open_client_id, spot_close_client_id,
        perp_close_client_id, spot_filled::text, perp_filled::text, spot_closed::text,
        perp_closed::text, state, funding_at, snapshot_before, snapshot_after, outcome,
        created_at, opened_at, closed_at
     FROM sandbox_cycles";

#[derive(sqlx::FromRow)]
struct CycleRow {
    id: Uuid,
    kind: String,
    venue: String,
    underlying: String,
    spot_symbol: Option<String>,
    perp_symbol: String,
    base_quantity: String,
    spot_open_client_id: Option<String>,
    perp_open_client_id: String,
    spot_close_client_id: Option<String>,
    perp_close_client_id: Option<String>,
    spot_filled: String,
    perp_filled: String,
    spot_closed: String,
    perp_closed: String,
    state: String,
    funding_at: Option<DateTime<Utc>>,
    snapshot_before: Option<Value>,
    snapshot_after: Option<Value>,
    outcome: Option<Value>,
    created_at: DateTime<Utc>,
    opened_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
}

impl TryFrom<CycleRow> for SandboxCycle {
    type Error = SandboxError;

    fn try_from(row: CycleRow) -> Result<Self, Self::Error> {
        let number = |value: &str| {
            value
                .parse::<Decimal>()
                .map_err(|_| SandboxError::Persistence)
        };
        Ok(Self {
            base_quantity: number(&row.base_quantity)?,
            spot_filled: number(&row.spot_filled)?,
            perp_filled: number(&row.perp_filled)?,
            spot_closed: number(&row.spot_closed)?,
            perp_closed: number(&row.perp_closed)?,
            id: row.id,
            kind: row.kind,
            venue: row.venue,
            underlying: row.underlying,
            spot_symbol: row.spot_symbol,
            perp_symbol: row.perp_symbol,
            spot_open_client_id: row.spot_open_client_id,
            perp_open_client_id: row.perp_open_client_id,
            spot_close_client_id: row.spot_close_client_id,
            perp_close_client_id: row.perp_close_client_id,
            state: row.state,
            funding_at: row.funding_at,
            snapshot_before: row.snapshot_before,
            snapshot_after: row.snapshot_after,
            outcome: row.outcome,
            created_at: row.created_at,
            opened_at: row.opened_at,
            closed_at: row.closed_at,
        })
    }
}

/// Size a hedge: the largest base quantity at or under `notional` that both
/// legs can represent, with marketable limits `slippage` through the touch.
///
/// # Errors
/// [`SandboxError::Instrument`] when the two lot steps cannot share a
/// quantity, and [`SandboxError::BelowMinimum`] when `notional` cannot meet
/// every minimum quantity and notional.
pub fn size_hedge(
    spot: &Instrument,
    perp: &Instrument,
    spot_ask: Decimal,
    perp_bid: Decimal,
    notional: Decimal,
    slippage: Decimal,
) -> Result<HedgePlan, SandboxError> {
    if spot_ask <= Decimal::ZERO || perp_bid <= Decimal::ZERO {
        return Err(SandboxError::Market("the book has no usable price".into()));
    }
    let (spot_step, perp_step) = (base_step(spot), base_step(perp));
    let step = spot_step.max(perp_step);
    if step <= Decimal::ZERO
        || (step % spot_step) != Decimal::ZERO
        || (step % perp_step) != Decimal::ZERO
    {
        return Err(SandboxError::Instrument(format!(
            "spot step {spot_step} and perpetual step {perp_step} share no quantity"
        )));
    }
    let required = ceil_to(
        base_minimum(spot)
            .max(base_minimum(perp))
            .max(spot.minimum_notional / spot_ask)
            .max(perp.minimum_notional / perp_bid),
        step,
    )
    .max(step);
    let quantity = floor_to(notional / spot_ask, step);
    if quantity < required {
        return Err(SandboxError::BelowMinimum {
            required_notional: (required * spot_ask * Decimal::new(105, 2))
                .round_dp_with_strategy(0, rust_decimal::RoundingStrategy::AwayFromZero),
        });
    }
    Ok(HedgePlan {
        quantity,
        spot_price: ceil_to(spot_ask * (Decimal::ONE + slippage), spot.price_tick),
        perp_price: floor_to(perp_bid * (Decimal::ONE - slippage), perp.price_tick),
    })
}

/// The smallest order quantity the instrument accepts at `price`.
#[must_use]
pub fn minimum_quantity(instrument: &Instrument, price: Decimal) -> Decimal {
    let step = base_step(instrument);
    let by_notional = if price > Decimal::ZERO {
        instrument.minimum_notional / price
    } else {
        Decimal::ZERO
    };
    ceil_to(base_minimum(instrument).max(by_notional), step).max(step)
}

fn base_step(instrument: &Instrument) -> Decimal {
    instrument.quantity_step * instrument.contract_multiplier
}

fn base_minimum(instrument: &Instrument) -> Decimal {
    instrument.minimum_quantity * instrument.contract_multiplier
}

/// Round down to a multiple of `step`.
#[must_use]
pub fn floor_to(value: Decimal, step: Decimal) -> Decimal {
    if step <= Decimal::ZERO {
        return value;
    }
    ((value / step).floor() * step).normalize()
}

/// Round up to a multiple of `step`.
#[must_use]
pub fn ceil_to(value: Decimal, step: Decimal) -> Decimal {
    if step <= Decimal::ZERO {
        return value;
    }
    ((value / step).ceil() * step).normalize()
}

fn filled(order: Option<&VenueOrder>) -> Decimal {
    order.map_or(Decimal::ZERO, |order| order.filled_quantity)
}

fn client_id(tag: &str) -> String {
    // Alphanumeric and 24 characters: within Bybit, Binance and OKX limits.
    format!("m4{tag}{}", &Uuid::new_v4().simple().to_string()[..20])
}

fn intent(
    leg: &SandboxLeg<'_>,
    instrument: &Instrument,
    side: OrderSide,
    quantity: Decimal,
    price: Decimal,
    reduce_only: bool,
    client_order_id: &str,
) -> CanonicalOrderIntent {
    CanonicalOrderIntent {
        account_id: leg.account.clone(),
        instrument: instrument.id.clone(),
        side,
        base_quantity: quantity,
        limit_price: price,
        time_in_force: "IOC".into(),
        reduce_only,
        client_order_id: client_order_id.into(),
    }
}

async fn query(
    leg: &SandboxLeg<'_>,
    client_order_id: &str,
) -> Result<Option<VenueOrder>, SandboxError> {
    leg.connector
        .query_order(
            &leg.account,
            &OrderLookup::ClientOrderId(client_order_id.into()),
        )
        .await
        .map_err(SandboxError::Connector)
}

/// The USDT-quoted instrument for `underlying` and `product`.
///
/// # Errors
/// [`SandboxError::Instrument`] when the venue does not list it or it is not
/// trading.
pub async fn find_instrument(
    connector: &dyn VenueConnector,
    underlying: &str,
    product: ProductType,
) -> Result<Instrument, SandboxError> {
    let instruments = connector
        .instruments()
        .await
        .map_err(SandboxError::Connector)?;
    instruments
        .into_iter()
        .find(|instrument| {
            instrument.id.product == product
                && instrument
                    .canonical_underlying
                    .eq_ignore_ascii_case(underlying)
                && instrument.quote_asset == "USDT"
                && instrument.trading
        })
        .ok_or_else(|| {
            SandboxError::Instrument(format!(
                "{} lists no trading {underlying}/USDT {}",
                connector.venue(),
                product.as_str()
            ))
        })
}

/// Best bid and best ask from the venue's first book snapshot.
///
/// # Errors
/// [`SandboxError::Market`] when the stream fails, times out, or the book is
/// empty on either side.
pub async fn top_of_book(
    connector: &dyn VenueConnector,
    instrument: &Instrument,
    timeout: Duration,
) -> Result<(Decimal, Decimal), SandboxError> {
    let venue = connector.venue();
    let first = tokio::time::timeout(timeout, async {
        let mut stream = connector
            .market_stream(vec![MarketSubscription {
                instrument: instrument.id.clone(),
                depth: 50,
            }])
            .await
            .map_err(|error| format!("{venue} market stream: {}", error.message))?;
        stream
            .next()
            .await
            .ok_or_else(|| format!("{venue} market stream ended"))?
            .map_err(|error| format!("{venue} market snapshot: {}", error.message))
    })
    .await
    .map_err(|_| SandboxError::Market(format!("{venue} book timed out")))?
    .map_err(SandboxError::Market)?;
    let MarketEvent::BookSnapshot { bids, asks, .. } = first else {
        return Err(SandboxError::Market(format!(
            "{venue} did not start with a book snapshot"
        )));
    };
    let bid = bids.iter().map(|level| level.price).max();
    let ask = asks.iter().map(|level| level.price).min();
    match (bid, ask) {
        (Some(bid), Some(ask)) if bid > Decimal::ZERO && ask >= bid => Ok((bid, ask)),
        _ => Err(SandboxError::Market(format!(
            "{venue} {} book is empty or crossed",
            instrument.id.symbol
        ))),
    }
}

async fn base_available(venue: &SandboxVenue<'_>, asset: &str) -> Decimal {
    venue
        .spot
        .connector
        .balances(&venue.spot.account)
        .await
        .ok()
        .and_then(|balances| {
            balances
                .into_iter()
                .find(|balance| balance.asset.eq_ignore_ascii_case(asset))
        })
        .map_or(Decimal::ZERO, |balance| balance.available)
}

/// Balances of both legs and perpetual positions, as exact decimal strings.
/// A failed read is recorded as an error rather than failing the run.
async fn snapshot(venue: &SandboxVenue<'_>) -> Value {
    let mut legs = serde_json::Map::new();
    for (name, leg) in [("spot", &venue.spot), ("perp", &venue.perp)] {
        let balances = match leg.connector.balances(&leg.account).await {
            Ok(balances) => Value::Array(
                balances
                    .into_iter()
                    .filter(|balance| !balance.total.is_zero())
                    .map(|balance| {
                        json!({
                            "asset": balance.asset,
                            "total": balance.total.to_string(),
                            "available": balance.available.to_string(),
                        })
                    })
                    .collect(),
            ),
            Err(error) => json!({"error": error.code}),
        };
        let positions = if name == "perp" {
            match leg.connector.positions(&leg.account).await {
                Ok(positions) => Value::Array(
                    positions
                        .into_iter()
                        .filter(|position| !position.quantity.is_zero())
                        .map(|position| {
                            json!({
                                "symbol": position.instrument.symbol,
                                "quantity": position.quantity.to_string(),
                                "entry_price": position.entry_price.to_string(),
                                "mark_price": position.mark_price.to_string(),
                            })
                        })
                        .collect(),
                ),
                Err(error) => json!({"error": error.code}),
            }
        } else {
            Value::Null
        };
        legs.insert(
            name.into(),
            json!({"balances": balances, "positions": positions}),
        );
    }
    legs.insert("taken_at".into(), json!(Utc::now()));
    Value::Object(legs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use funding_arb_connectors::InstrumentId;
    use funding_arb_domain::market::Venue;

    fn instrument(product: ProductType, step: &str, minimum: &str, notional: &str) -> Instrument {
        Instrument {
            id: InstrumentId {
                venue: Venue::Binance,
                symbol: "ETHUSDT".into(),
                product,
            },
            canonical_underlying: "ETH".into(),
            quote_asset: "USDT".into(),
            settlement_asset: "USDT".into(),
            margin_asset: "USDT".into(),
            contract_multiplier: Decimal::ONE,
            price_tick: Decimal::new(1, 2),
            quantity_step: step.parse().expect("step"),
            minimum_quantity: minimum.parse().expect("minimum"),
            maximum_quantity: None,
            minimum_notional: notional.parse().expect("notional"),
            funding_interval_hours: None,
            funding_cap: None,
            funding_floor: None,
            maximum_leverage: None,
            trading: true,
            metadata_version: "test".into(),
            venue_updated_at: None,
            received_at: Utc::now(),
        }
    }

    #[test]
    fn a_hedge_uses_the_coarser_step_and_crosses_the_touch() {
        let spot = instrument(ProductType::Spot, "0.0001", "0.0001", "5");
        let perp = instrument(ProductType::LinearPerpetual, "0.001", "0.001", "20");
        let plan = size_hedge(
            &spot,
            &perp,
            Decimal::new(300_000, 2),
            Decimal::new(299_900, 2),
            Decimal::new(150, 0),
            DEFAULT_SLIPPAGE,
        )
        .expect("plan");
        assert_eq!(plan.quantity, Decimal::new(5, 2));
        assert_eq!(plan.spot_price, Decimal::new(3030, 0));
        assert_eq!(plan.perp_price, Decimal::new(296_901, 2));
    }

    #[test]
    fn a_notional_under_the_minimum_names_the_required_amount() {
        let spot = instrument(ProductType::Spot, "0.0001", "0.0001", "5");
        let perp = instrument(ProductType::LinearPerpetual, "0.001", "0.001", "100");
        let error = size_hedge(
            &spot,
            &perp,
            Decimal::new(3000, 0),
            Decimal::new(3000, 0),
            Decimal::new(50, 0),
            DEFAULT_SLIPPAGE,
        )
        .expect_err("below minimum");
        let SandboxError::BelowMinimum { required_notional } = error else {
            panic!("expected BelowMinimum, got {error:?}");
        };
        // 100 USDT / 3000 = 0.0333.. → 0.034 ETH; 0.034 × 3000 × 1.05 = 107.1 → 108.
        assert_eq!(required_notional, Decimal::new(108, 0));
    }

    #[test]
    fn contract_sized_perpetuals_are_measured_in_base_units() {
        let spot = instrument(ProductType::Spot, "0.000001", "0.0001", "0");
        let mut perp = instrument(ProductType::LinearPerpetual, "0.01", "0.01", "0");
        // OKX ETH-USDT-SWAP: 0.1 ETH per contract, 0.01-contract lots.
        perp.contract_multiplier = Decimal::new(1, 1);
        let plan = size_hedge(
            &spot,
            &perp,
            Decimal::new(3000, 0),
            Decimal::new(3000, 0),
            Decimal::new(150, 0),
            Decimal::ZERO,
        )
        .expect("plan");
        assert_eq!(plan.quantity, Decimal::new(5, 2));
        assert_eq!(
            minimum_quantity(&perp, Decimal::new(3000, 0)),
            Decimal::new(1, 3)
        );
    }

    #[test]
    fn steps_that_share_no_quantity_are_refused() {
        let spot = instrument(ProductType::Spot, "0.003", "0.003", "0");
        let perp = instrument(ProductType::LinearPerpetual, "0.002", "0.002", "0");
        assert!(matches!(
            size_hedge(
                &spot,
                &perp,
                Decimal::new(3000, 0),
                Decimal::new(3000, 0),
                Decimal::new(150, 0),
                Decimal::ZERO,
            ),
            Err(SandboxError::Instrument(_))
        ));
    }

    #[test]
    fn rounding_stays_on_the_grid() {
        assert_eq!(
            floor_to(Decimal::new(12_345, 4), Decimal::new(1, 3)),
            Decimal::new(1234, 3)
        );
        assert_eq!(
            ceil_to(Decimal::new(12_341, 4), Decimal::new(1, 3)),
            Decimal::new(1235, 3)
        );
        assert_eq!(floor_to(Decimal::ONE, Decimal::ZERO), Decimal::ONE);
    }

    #[test]
    fn client_ids_fit_every_venue() {
        let id = client_id("so");
        assert_eq!(id.len(), 24);
        assert!(
            id.chars()
                .all(|character| character.is_ascii_alphanumeric())
        );
    }

    #[tokio::test]
    async fn the_driver_refuses_every_mode_but_sandbox() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused@localhost/unused")
            .expect("lazy pool");
        for mode in [TradingMode::Paper, TradingMode::Shadow, TradingMode::Live] {
            assert!(matches!(
                SandboxDriver::new(pool.clone(), mode),
                Err(SandboxError::ModeForbidden)
            ));
        }
    }
}
