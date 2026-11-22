//! Keyless public market data for live-data PAPER (DEC-0016).
//!
//! [`PublicMarketSource`] reads instruments, funding and order books from the
//! production *public* endpoints of each venue. Connectors are constructed with
//! no credentials, so this module cannot open a private stream, read an
//! account, or submit an order: those calls need credentials the connectors do
//! not hold. It never touches the credential store or the account registry.
//!
//! A venue that fails or times out contributes no legs and is reported as
//! degraded in [`MarketSnapshot::venues`]. There is no fallback to fixture data.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use funding_arb_connectors::{
    BinanceConnector, BybitConnector, HealthStatus, Instrument, MarketEvent, MarketSubscription,
    OkxConnector, PriceLevel, ProductType, VenueConnector, VenueEnvironment,
};
use funding_arb_domain::market::{FundingObservation, InstrumentRef, Venue};
use funding_arb_scanner_service::fixtures::{InstrumentKind, MarketLeg, MarketStatus};
pub use funding_arb_scanner_service::market::{
    BookSide, FeedStatus, LegBook, MarketSnapshot, VenueFeedStatus,
};
use futures_util::StreamExt;
use rust_decimal::Decimal;

/// Underlyings scanned when `PERPETO_PAPER_UNDERLYINGS` is unset.
pub const DEFAULT_UNDERLYINGS: &[&str] = &["BTC", "ETH"];
/// Book depth requested per instrument.
pub const DEFAULT_DEPTH: u32 = 50;
/// Ceiling on a latency-sensitive venue call: health, book or funding.
pub const DEFAULT_VENUE_TIMEOUT: Duration = Duration::from_secs(10);
/// Ceiling on an instrument-metadata call. These responses are large (Binance
/// `exchangeInfo` is about 18 MB), so they get a longer budget.
pub const DEFAULT_METADATA_TIMEOUT: Duration = Duration::from_secs(45);
/// How long cached instrument metadata is reused before it is refetched.
pub const DEFAULT_METADATA_TTL_SECS: i64 = 900;

/// Configuration for the public source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicMarketConfig {
    /// Canonical underlyings to scan, upper-case (for example `BTC`).
    pub underlyings: Vec<String>,
    /// Order-book depth requested per instrument.
    pub depth: u32,
    /// Timeout applied to each health, book and funding call.
    pub venue_timeout: Duration,
    /// Timeout applied to each instrument-metadata call.
    pub metadata_timeout: Duration,
    /// How long instrument metadata is cached per venue.
    pub metadata_ttl: chrono::Duration,
}

impl Default for PublicMarketConfig {
    fn default() -> Self {
        Self {
            underlyings: DEFAULT_UNDERLYINGS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            depth: DEFAULT_DEPTH,
            venue_timeout: DEFAULT_VENUE_TIMEOUT,
            metadata_timeout: DEFAULT_METADATA_TIMEOUT,
            metadata_ttl: chrono::Duration::seconds(DEFAULT_METADATA_TTL_SECS),
        }
    }
}

impl PublicMarketConfig {
    /// Parse the value of `PERPETO_PAPER_UNDERLYINGS`: a comma-separated list of
    /// alphanumeric symbols. Unset means the defaults.
    ///
    /// # Errors
    /// Returns a message when the list is empty or contains an invalid symbol.
    pub fn with_underlyings(mut self, value: Option<&str>) -> Result<Self, String> {
        let Some(raw) = value else {
            return Ok(self);
        };
        let mut underlyings = Vec::new();
        for part in raw.split(',') {
            let symbol = part.trim().to_ascii_uppercase();
            if symbol.is_empty()
                || !symbol
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
            {
                return Err(format!(
                    "invalid underlying in PERPETO_PAPER_UNDERLYINGS: {part:?}"
                ));
            }
            if !underlyings.contains(&symbol) {
                underlyings.push(symbol);
            }
        }
        if underlyings.is_empty() {
            return Err("PERPETO_PAPER_UNDERLYINGS lists no underlying".to_owned());
        }
        self.underlyings = underlyings;
        Ok(self)
    }
}

/// Instrument metadata for one venue, already filtered to the universe.
#[derive(Clone)]
struct CachedInstruments {
    fetched_at: DateTime<Utc>,
    instruments: Vec<Instrument>,
}

/// Reads keyless public market data from every venue.
///
/// Instrument metadata changes rarely and is expensive to fetch, so it is
/// cached per venue for [`PublicMarketConfig::metadata_ttl`]. Books and funding
/// are read fresh on every snapshot.
pub struct PublicMarketSource {
    connectors: Vec<Arc<dyn VenueConnector>>,
    config: PublicMarketConfig,
    metadata: Mutex<HashMap<Venue, CachedInstruments>>,
}

impl PublicMarketSource {
    /// Binance, Bybit and OKX production public endpoints, with no credentials.
    #[must_use]
    pub fn production(config: PublicMarketConfig) -> Self {
        Self::new(
            vec![
                Arc::new(BinanceConnector::new(VenueEnvironment::Production, None)),
                Arc::new(BybitConnector::new(VenueEnvironment::Production, None)),
                Arc::new(OkxConnector::new(VenueEnvironment::Production, None)),
            ],
            config,
        )
    }

    /// A source over the given connectors.
    #[must_use]
    pub fn new(connectors: Vec<Arc<dyn VenueConnector>>, config: PublicMarketConfig) -> Self {
        Self {
            connectors,
            config,
            metadata: Mutex::new(HashMap::new()),
        }
    }

    /// The configuration in use.
    #[must_use]
    pub fn config(&self) -> &PublicMarketConfig {
        &self.config
    }

    /// Read every venue concurrently. A failing venue is reported, never fatal.
    pub async fn snapshot(&self, now: DateTime<Utc>) -> MarketSnapshot {
        let reads = self.connectors.iter().map(|connector| async move {
            let venue = connector.venue();
            (venue, self.read_venue(connector.as_ref(), now).await)
        });
        let mut snapshot = MarketSnapshot {
            taken_at: now,
            legs: Vec::new(),
            books: Vec::new(),
            venues: Vec::new(),
        };
        for (venue, result) in futures_util::future::join_all(reads).await {
            match result {
                Ok(read) => {
                    let observed_at = read
                        .legs
                        .iter()
                        .map(|leg| leg.observation.observed_at)
                        .max();
                    snapshot.venues.push(VenueFeedStatus {
                        venue,
                        status: if read.healthy {
                            FeedStatus::Healthy
                        } else {
                            FeedStatus::Degraded
                        },
                        observed_at,
                        error: None,
                    });
                    snapshot.legs.extend(read.legs);
                    snapshot.books.extend(read.books);
                }
                Err(error) => snapshot.venues.push(VenueFeedStatus {
                    venue,
                    status: FeedStatus::Unavailable,
                    observed_at: None,
                    error: Some(error),
                }),
            }
        }
        snapshot
    }

    /// The venue's universe instruments, from cache while fresh.
    async fn instruments(
        &self,
        connector: &dyn VenueConnector,
        now: DateTime<Utc>,
    ) -> Result<Vec<Instrument>, String> {
        let venue = connector.venue();
        let cached = self
            .metadata
            .lock()
            .ok()
            .and_then(|cache| cache.get(&venue).cloned());
        if let Some(cached) = cached
            && now - cached.fetched_at < self.config.metadata_ttl
        {
            return Ok(cached.instruments);
        }
        let all = within(self.config.metadata_timeout, "instruments", async {
            connector
                .instruments()
                .await
                .map_err(|error| format!("{venue} instruments: {error}"))
        })
        .await?;
        let instruments: Vec<Instrument> = all
            .into_iter()
            .filter(|instrument| {
                instrument.trading
                    && self
                        .config
                        .underlyings
                        .contains(&instrument.canonical_underlying)
            })
            .collect();
        if let Ok(mut cache) = self.metadata.lock() {
            cache.insert(
                venue,
                CachedInstruments {
                    fetched_at: now,
                    instruments: instruments.clone(),
                },
            );
        }
        Ok(instruments)
    }

    async fn read_venue(
        &self,
        connector: &dyn VenueConnector,
        now: DateTime<Utc>,
    ) -> Result<VenueRead, String> {
        let venue = connector.venue();
        let health = within(self.config.venue_timeout, "health", async {
            connector
                .health()
                .await
                .map_err(|error| format!("{venue} health: {error}"))
        })
        .await?;
        let status = match health.status {
            HealthStatus::Healthy => MarketStatus::Healthy,
            _ => MarketStatus::Degraded,
        };
        let instruments = self.instruments(connector, now).await?;
        let mut read = VenueRead {
            healthy: status == MarketStatus::Healthy,
            legs: Vec::new(),
            books: Vec::new(),
        };
        for instrument in &instruments {
            let (leg, book) = measure_leg(
                connector,
                instrument,
                status,
                self.config.depth,
                self.config.venue_timeout,
            )
            .await?;
            read.legs.push(leg);
            read.books.push(book);
        }
        Ok(read)
    }
}

struct VenueRead {
    healthy: bool,
    legs: Vec<MarketLeg>,
    books: Vec<LegBook>,
}

async fn within<T>(
    timeout: Duration,
    label: &str,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| format!("{label} timed out"))?
}

/// Measure one instrument: a book snapshot plus, for perpetuals, its current
/// funding. Shared by live-data PAPER and the credentialed SANDBOX/SHADOW
/// scan so the two paths cannot diverge.
///
/// # Errors
/// Returns a message when the venue call fails, times out, or returns an
/// unusable book.
pub async fn measure_leg(
    connector: &dyn VenueConnector,
    instrument: &Instrument,
    status: MarketStatus,
    depth: u32,
    timeout: Duration,
) -> Result<(MarketLeg, LegBook), String> {
    let venue = connector.venue();
    let event = within(timeout, "market stream", async {
        let mut stream = connector
            .market_stream(vec![MarketSubscription {
                instrument: instrument.id.clone(),
                depth,
            }])
            .await
            .map_err(|error| format!("{venue} market stream: {error}"))?;
        stream
            .next()
            .await
            .ok_or_else(|| format!("{venue} market stream ended"))?
            .map_err(|error| format!("{venue} market snapshot: {error}"))
    })
    .await?;
    let SnapshotLevels {
        bids,
        asks,
        received_at,
    } = book_levels(event)?;
    let (book_mark, available_notional) =
        book_measurement(&bids, &asks, instrument.contract_multiplier)?;
    let (funding_rate, interval_hours, mark_price, index_price, next_funding_at) =
        if instrument.id.product == ProductType::LinearPerpetual {
            let funding = within(timeout, "funding", async {
                connector
                    .current_funding(&instrument.id)
                    .await
                    .map_err(|error| format!("{venue} funding: {error}"))
            })
            .await?;
            (
                funding.rate,
                funding.interval_hours,
                funding.mark_price,
                funding.index_price,
                funding.next_funding_at,
            )
        } else {
            (
                Decimal::ZERO,
                8,
                book_mark,
                book_mark,
                received_at + chrono::Duration::hours(8),
            )
        };
    let leg = MarketLeg {
        kind: if instrument.id.product == ProductType::Spot {
            InstrumentKind::Spot
        } else {
            InstrumentKind::Perp
        },
        status,
        observation: FundingObservation {
            instrument: InstrumentRef {
                venue: instrument.id.venue,
                symbol: instrument.id.symbol.clone(),
                underlying: instrument.canonical_underlying.clone(),
            },
            funding_rate,
            interval_hours,
            mark_price,
            index_price,
            available_notional,
            observed_at: received_at,
            next_funding_at,
        },
    };
    let book = LegBook {
        venue: instrument.id.venue,
        symbol: instrument.id.symbol.clone(),
        contract_multiplier: instrument.contract_multiplier,
        bids: BookSide {
            levels: bids
                .iter()
                .map(|level| (level.price, level.quantity))
                .collect(),
        },
        asks: BookSide {
            levels: asks
                .iter()
                .map(|level| (level.price, level.quantity))
                .collect(),
        },
    };
    Ok((leg, book))
}

/// The two sides of a book snapshot and when it was received.
struct SnapshotLevels {
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
    received_at: DateTime<Utc>,
}

fn book_levels(event: MarketEvent) -> Result<SnapshotLevels, String> {
    let MarketEvent::BookSnapshot {
        bids,
        asks,
        received_at,
        ..
    } = event
    else {
        return Err("connector did not provide a book snapshot".into());
    };
    Ok(SnapshotLevels {
        bids,
        asks,
        received_at,
    })
}

/// Mid price and the executable notional on the thinner side of the book.
///
/// # Errors
/// Returns a message for an empty or crossed book.
pub fn book_measurement(
    bids: &[PriceLevel],
    asks: &[PriceLevel],
    contract_multiplier: Decimal,
) -> Result<(Decimal, Decimal), String> {
    let best_bid = bids
        .first()
        .ok_or_else(|| "connector book has no bids".to_owned())?;
    let best_ask = asks
        .first()
        .ok_or_else(|| "connector book has no asks".to_owned())?;
    if best_bid.price >= best_ask.price {
        return Err("connector book is crossed".into());
    }
    let mark = (best_bid.price + best_ask.price) / Decimal::new(2, 0);
    let side = |levels: &[PriceLevel]| -> Decimal {
        levels
            .iter()
            .map(|level| level.price * level.quantity * contract_multiplier)
            .sum()
    };
    Ok((mark, side(bids).min(side(asks))))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use chrono::TimeZone;
    use funding_arb_connectors::{
        AccountId, Balance, CancelAck, CancelRequest, CanonicalOrderIntent, ConnectorError,
        ConnectorErrorKind, ConnectorResult, FeeSchedule, FillPage, FundingLedgerEntry,
        FundingQuote, InstrumentId, MarketEventStream, OrderAck, OrderLookup, PreparedOrder,
        PrivateEventStream, TimeRange, VenueCapabilities, VenueHealth, VenueOrder, VenuePosition,
        VenueTimestamp,
    };
    use rust_decimal::Decimal;

    use super::*;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 22, 12, 0, 0)
            .single()
            .unwrap_or_default()
    }

    fn dec(value: &str) -> Decimal {
        value.parse().unwrap_or_default()
    }

    fn level(price: &str, quantity: &str) -> PriceLevel {
        PriceLevel {
            price: dec(price),
            quantity: dec(quantity),
        }
    }

    fn unsupported<T>() -> ConnectorResult<T> {
        Err(ConnectorError {
            kind: ConnectorErrorKind::Unsupported,
            code: "UNSUPPORTED".into(),
            message: "public fake".into(),
            retry_after_ms: None,
        })
    }

    #[derive(Clone, Copy)]
    enum Behaviour {
        Normal,
        DegradedHealth,
        HealthFails,
        StreamHangs,
        CrossedBook,
    }

    /// A scripted venue: BTC and ETH spot and perpetuals plus one non-trading
    /// and one out-of-universe instrument. Every private call is unsupported.
    struct FakeConnector {
        venue: Venue,
        behaviour: Behaviour,
        stream_calls: AtomicUsize,
        instrument_calls: AtomicUsize,
        subscribed: Mutex<Vec<String>>,
    }

    impl FakeConnector {
        fn new(venue: Venue, behaviour: Behaviour) -> Self {
            Self {
                venue,
                behaviour,
                stream_calls: AtomicUsize::new(0),
                instrument_calls: AtomicUsize::new(0),
                subscribed: Mutex::new(Vec::new()),
            }
        }

        fn instrument(&self, underlying: &str, product: ProductType, trading: bool) -> Instrument {
            Instrument {
                id: InstrumentId {
                    venue: self.venue,
                    symbol: format!("{underlying}USDT-{}", product.as_str()),
                    product,
                },
                canonical_underlying: underlying.into(),
                quote_asset: "USDT".into(),
                settlement_asset: "USDT".into(),
                margin_asset: "USDT".into(),
                contract_multiplier: Decimal::ONE,
                price_tick: dec("0.1"),
                quantity_step: dec("0.001"),
                minimum_quantity: dec("0.001"),
                maximum_quantity: None,
                minimum_notional: dec("5"),
                funding_interval_hours: Some(8),
                funding_cap: None,
                funding_floor: None,
                maximum_leverage: None,
                trading,
                metadata_version: "1".into(),
                venue_updated_at: None,
                received_at: at(),
            }
        }
    }

    #[async_trait]
    impl VenueConnector for FakeConnector {
        fn venue(&self) -> Venue {
            self.venue
        }

        fn capabilities(&self) -> VenueCapabilities {
            VenueCapabilities {
                products: vec![ProductType::Spot, ProductType::LinearPerpetual],
                market_stream: true,
                private_stream: false,
                funding_history: false,
                order_submission: false,
                borrow: false,
                internal_transfer: false,
            }
        }

        async fn health(&self) -> ConnectorResult<VenueHealth> {
            if matches!(self.behaviour, Behaviour::HealthFails) {
                return Err(ConnectorError {
                    kind: ConnectorErrorKind::Transient,
                    code: "HTTP_503".into(),
                    message: "venue down".into(),
                    retry_after_ms: None,
                });
            }
            Ok(VenueHealth {
                venue: self.venue,
                environment: VenueEnvironment::Production,
                status: if matches!(self.behaviour, Behaviour::DegradedHealth) {
                    HealthStatus::Degraded
                } else {
                    HealthStatus::Healthy
                },
                server_time: at(),
                observed_at: at(),
                clock_drift_ms: 0,
                rate_limit_remaining: None,
                detail: None,
            })
        }

        async fn server_time(&self) -> ConnectorResult<VenueTimestamp> {
            unsupported()
        }

        async fn instruments(&self) -> ConnectorResult<Vec<Instrument>> {
            self.instrument_calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![
                self.instrument("BTC", ProductType::Spot, true),
                self.instrument("BTC", ProductType::LinearPerpetual, true),
                self.instrument("ETH", ProductType::LinearPerpetual, true),
                self.instrument("SOL", ProductType::LinearPerpetual, true),
                self.instrument("ETH", ProductType::Spot, false),
            ])
        }

        async fn market_stream(
            &self,
            subscriptions: Vec<MarketSubscription>,
        ) -> ConnectorResult<MarketEventStream> {
            self.stream_calls.fetch_add(1, Ordering::SeqCst);
            let Some(subscription) = subscriptions.into_iter().next() else {
                return unsupported();
            };
            if let Ok(mut subscribed) = self.subscribed.lock() {
                subscribed.push(subscription.instrument.symbol.clone());
            }
            if matches!(self.behaviour, Behaviour::StreamHangs) {
                return Ok(Box::pin(futures_util::stream::pending()));
            }
            let (bids, asks) = if matches!(self.behaviour, Behaviour::CrossedBook) {
                (vec![level("101", "1")], vec![level("100", "1")])
            } else {
                (
                    vec![level("99", "2"), level("98", "3")],
                    vec![level("101", "1"), level("102", "4")],
                )
            };
            let event = MarketEvent::BookSnapshot {
                instrument: subscription.instrument,
                sequence: 1,
                bids,
                asks,
                venue_time: at(),
                received_at: at(),
                checksum: None,
            };
            Ok(Box::pin(futures_util::stream::iter(vec![Ok(event)])))
        }

        async fn private_stream(
            &self,
            _account: &AccountId,
        ) -> ConnectorResult<PrivateEventStream> {
            unsupported()
        }

        async fn fee_schedule(&self, _account: &AccountId) -> ConnectorResult<Vec<FeeSchedule>> {
            unsupported()
        }

        async fn current_funding(
            &self,
            instrument: &InstrumentId,
        ) -> ConnectorResult<FundingQuote> {
            Ok(FundingQuote {
                instrument: instrument.clone(),
                rate: dec("0.0001"),
                interval_hours: 8,
                mark_price: dec("100.5"),
                index_price: dec("100.4"),
                next_funding_at: at() + chrono::Duration::hours(4),
                observed_at: at(),
            })
        }

        async fn funding_history(
            &self,
            _instrument: &InstrumentId,
            _range: &TimeRange,
        ) -> ConnectorResult<Vec<FundingQuote>> {
            unsupported()
        }

        async fn balances(&self, _account: &AccountId) -> ConnectorResult<Vec<Balance>> {
            unsupported()
        }

        async fn positions(&self, _account: &AccountId) -> ConnectorResult<Vec<VenuePosition>> {
            unsupported()
        }

        async fn open_orders(&self, _account: &AccountId) -> ConnectorResult<Vec<VenueOrder>> {
            unsupported()
        }

        async fn fills(
            &self,
            _account: &AccountId,
            _cursor: Option<&str>,
        ) -> ConnectorResult<FillPage> {
            unsupported()
        }

        async fn funding_ledger(
            &self,
            _account: &AccountId,
            _range: &TimeRange,
        ) -> ConnectorResult<Vec<FundingLedgerEntry>> {
            unsupported()
        }

        async fn prepare_order(
            &self,
            _order: &CanonicalOrderIntent,
        ) -> ConnectorResult<PreparedOrder> {
            unsupported()
        }

        async fn submit_order(&self, _order: &PreparedOrder) -> ConnectorResult<OrderAck> {
            unsupported()
        }

        async fn cancel_order(&self, _request: &CancelRequest) -> ConnectorResult<CancelAck> {
            unsupported()
        }

        async fn query_order(
            &self,
            _account: &AccountId,
            _key: &OrderLookup,
        ) -> ConnectorResult<Option<VenueOrder>> {
            unsupported()
        }
    }

    fn source(connectors: Vec<Arc<dyn VenueConnector>>) -> PublicMarketSource {
        PublicMarketSource::new(
            connectors,
            PublicMarketConfig {
                venue_timeout: Duration::from_millis(200),
                metadata_timeout: Duration::from_millis(200),
                ..PublicMarketConfig::default()
            },
        )
    }

    #[tokio::test]
    async fn builds_legs_for_tradeable_instruments_in_the_universe_only() {
        let fake = Arc::new(FakeConnector::new(Venue::Bybit, Behaviour::Normal));
        let snapshot = source(vec![fake.clone()]).snapshot(at()).await;
        let symbols: Vec<_> = snapshot
            .legs
            .iter()
            .map(|leg| leg.observation.instrument.symbol.as_str())
            .collect();
        assert_eq!(
            symbols,
            [
                "BTCUSDT-SPOT",
                "BTCUSDT-LINEAR_PERPETUAL",
                "ETHUSDT-LINEAR_PERPETUAL"
            ]
        );
        // SOL (outside the universe) and the halted ETH spot are never streamed.
        assert_eq!(fake.stream_calls.load(Ordering::SeqCst), 3);
        assert_eq!(snapshot.books.len(), 3);
        assert_eq!(snapshot.venues.len(), 1);
        assert_eq!(snapshot.venues[0].status, FeedStatus::Healthy);
        assert_eq!(snapshot.venues[0].observed_at, Some(at()));
    }

    #[tokio::test]
    async fn perpetual_legs_carry_funding_and_spot_legs_carry_none() {
        let snapshot = source(vec![Arc::new(FakeConnector::new(
            Venue::Okx,
            Behaviour::Normal,
        ))])
        .snapshot(at())
        .await;
        let spot = &snapshot.legs[0];
        assert_eq!(spot.kind, InstrumentKind::Spot);
        assert_eq!(spot.observation.funding_rate, Decimal::ZERO);
        assert_eq!(spot.observation.mark_price, dec("100"));
        let perp = &snapshot.legs[1];
        assert_eq!(perp.kind, InstrumentKind::Perp);
        assert_eq!(perp.observation.funding_rate, dec("0.0001"));
        assert_eq!(perp.observation.mark_price, dec("100.5"));
        assert_eq!(
            perp.observation.next_funding_at,
            at() + chrono::Duration::hours(4)
        );
    }

    #[tokio::test]
    async fn stores_the_book_best_level_first_for_paper_fills() {
        let snapshot = source(vec![Arc::new(FakeConnector::new(
            Venue::Binance,
            Behaviour::Normal,
        ))])
        .snapshot(at())
        .await;
        let book = &snapshot.books[0];
        assert_eq!(book.venue, Venue::Binance);
        assert_eq!(
            book.bids.levels,
            vec![(dec("99"), dec("2")), (dec("98"), dec("3"))]
        );
        assert_eq!(
            book.asks.levels,
            vec![(dec("101"), dec("1")), (dec("102"), dec("4"))]
        );
        // Thinner side: bids 99*2 + 98*3 = 492; asks 101 + 408 = 509.
        assert_eq!(snapshot.legs[0].observation.available_notional, dec("492"));
    }

    #[tokio::test]
    async fn degraded_venue_health_marks_legs_degraded_but_keeps_data() {
        let snapshot = source(vec![Arc::new(FakeConnector::new(
            Venue::Bybit,
            Behaviour::DegradedHealth,
        ))])
        .snapshot(at())
        .await;
        assert_eq!(snapshot.venues[0].status, FeedStatus::Degraded);
        assert!(
            snapshot
                .legs
                .iter()
                .all(|leg| leg.status == MarketStatus::Degraded)
        );
    }

    #[tokio::test]
    async fn a_failing_venue_is_reported_and_does_not_block_the_others() {
        let snapshot = source(vec![
            Arc::new(FakeConnector::new(Venue::Bybit, Behaviour::HealthFails)),
            Arc::new(FakeConnector::new(Venue::Okx, Behaviour::Normal)),
        ])
        .snapshot(at())
        .await;
        let bybit = &snapshot.venues[0];
        assert_eq!(bybit.status, FeedStatus::Unavailable);
        assert_eq!(bybit.observed_at, None);
        assert!(
            bybit
                .error
                .as_deref()
                .is_some_and(|error| error.contains("venue down"))
        );
        assert_eq!(snapshot.venues[1].status, FeedStatus::Healthy);
        assert!(
            snapshot
                .legs
                .iter()
                .all(|leg| leg.observation.instrument.venue == Venue::Okx)
        );
        assert_eq!(snapshot.legs.len(), 3);
    }

    #[tokio::test]
    async fn a_hanging_stream_times_out_instead_of_stalling_the_scan() {
        let snapshot = source(vec![Arc::new(FakeConnector::new(
            Venue::Okx,
            Behaviour::StreamHangs,
        ))])
        .snapshot(at())
        .await;
        assert_eq!(snapshot.venues[0].status, FeedStatus::Unavailable);
        assert!(
            snapshot.venues[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("timed out"))
        );
        assert!(snapshot.legs.is_empty());
    }

    #[tokio::test]
    async fn a_crossed_book_makes_the_venue_unavailable() {
        let snapshot = source(vec![Arc::new(FakeConnector::new(
            Venue::Okx,
            Behaviour::CrossedBook,
        ))])
        .snapshot(at())
        .await;
        assert_eq!(snapshot.venues[0].status, FeedStatus::Unavailable);
        assert!(snapshot.legs.is_empty());
    }

    #[tokio::test]
    async fn instrument_metadata_is_cached_until_the_ttl_expires() {
        let fake = Arc::new(FakeConnector::new(Venue::Binance, Behaviour::Normal));
        let source = source(vec![fake.clone()]);
        source.snapshot(at()).await;
        source.snapshot(at() + chrono::Duration::seconds(60)).await;
        assert_eq!(fake.instrument_calls.load(Ordering::SeqCst), 1);
        // Books are always read fresh: 3 legs per snapshot.
        assert_eq!(fake.stream_calls.load(Ordering::SeqCst), 6);
        source
            .snapshot(at() + chrono::Duration::seconds(DEFAULT_METADATA_TTL_SECS))
            .await;
        assert_eq!(fake.instrument_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failed_venue_does_not_poison_the_metadata_cache() {
        let fake = Arc::new(FakeConnector::new(Venue::Bybit, Behaviour::HealthFails));
        let source = source(vec![fake.clone()]);
        source.snapshot(at()).await;
        assert_eq!(fake.instrument_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn book_measurement_rejects_empty_and_crossed_books() {
        assert!(book_measurement(&[], &[level("1", "1")], Decimal::ONE).is_err());
        assert!(book_measurement(&[level("1", "1")], &[], Decimal::ONE).is_err());
        assert!(book_measurement(&[level("2", "1")], &[level("2", "1")], Decimal::ONE).is_err());
        let (mark, notional) =
            book_measurement(&[level("99", "1")], &[level("101", "1")], dec("0.01"))
                .unwrap_or_default();
        assert_eq!(mark, dec("100"));
        assert_eq!(notional, dec("0.99"));
    }

    #[test]
    fn underlyings_parse_normalise_and_deduplicate() {
        let config = PublicMarketConfig::default();
        assert_eq!(
            config
                .clone()
                .with_underlyings(None)
                .map(|value| value.underlyings),
            Ok(vec!["BTC".to_owned(), "ETH".to_owned()])
        );
        assert_eq!(
            config
                .clone()
                .with_underlyings(Some(" sol, btc ,SOL"))
                .map(|value| value.underlyings),
            Ok(vec!["SOL".to_owned(), "BTC".to_owned()])
        );
        assert!(config.clone().with_underlyings(Some("")).is_err());
        assert!(config.clone().with_underlyings(Some("BTC,,ETH")).is_err());
        assert!(config.with_underlyings(Some("BTC-USD")).is_err());
    }

    #[test]
    fn the_public_source_never_touches_credentials() {
        // The module must not reach the secret store or the credentialed
        // registry; this guards the DEC-0016 boundary at the source level.
        let source = include_str!("public_market.rs");
        let body = source.split("#[cfg(test)]").next().unwrap_or_default();
        for forbidden in [
            "SecretStore",
            "ConnectorRegistry",
            "ConnectorCredentials",
            "private_stream(",
        ] {
            assert!(
                !body.contains(forbidden),
                "public_market.rs references {forbidden}"
            );
        }
    }
}
