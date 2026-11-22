//! Opening and supervising paper positions against a market snapshot.
//!
//! Every operation takes the [`MarketSnapshot`] a scan was ranked from: the
//! fixture snapshot in deterministic mode, or the live public snapshot in
//! live-data PAPER (DEC-0016). The route is re-ranked from the snapshot's legs,
//! and each leg fills at the VWAP of walking its captured book when one exists,
//! or at the mark crossed by the modeled slippage when it does not (fixture
//! snapshots carry no books). Given the same snapshot, opening is reproducible.

use std::fmt;

use chrono::{DateTime, Utc};
use funding_arb_domain::market::{RouteType, Venue};
use funding_arb_domain::position::{
    ArbitragePosition, ExitDecision, ExitReason, LegExecution, PaperFillConfig, PaperFillScenario,
    close_paper, evaluate_exit, open_paper_with_scenario, re_hedge,
};
use funding_arb_scanner_service::fixtures::{self, InstrumentKind, MarketLeg, MarketStatus};
use funding_arb_scanner_service::{BookSide, LegBook, MarketSnapshot, Scanner};
use rust_decimal::Decimal;
use uuid::Uuid;

/// Why a paper open could not be produced.
#[derive(Debug)]
pub enum OpenError {
    /// No opportunity with the requested `route_id` is in the latest scan.
    RouteNotFound,
    /// A leg of the matched route has no executable market leg.
    LegNotFound,
    /// A leg's venue feed is degraded, so no new exposure is taken on it.
    LegDegraded,
    /// A captured book is too thin to fill the requested size.
    InsufficientDepth,
    /// The market fixtures could not be loaded.
    Fixture(fixtures::FixtureError),
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RouteNotFound => formatter.write_str("route not found in the latest scan"),
            Self::LegNotFound => formatter.write_str("executable leg not found for the route"),
            Self::LegDegraded => formatter.write_str("a leg's venue feed is degraded"),
            Self::InsufficientDepth => {
                formatter.write_str("order book depth cannot fill the requested size")
            }
            Self::Fixture(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// Why supervision could not produce a safe updated position.
#[derive(Debug)]
pub enum SuperviseError {
    /// A current leg could not be resolved from the market fixtures.
    LegNotFound,
    /// The position's original route no longer exists in the fixtures.
    RouteNotFound,
    /// The market fixtures could not be loaded.
    Fixture(fixtures::FixtureError),
}

impl fmt::Display for SuperviseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LegNotFound => formatter.write_str("current position leg not found"),
            Self::RouteNotFound => formatter.write_str("position route not found"),
            Self::Fixture(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for SuperviseError {}

/// The paper execution service: opens positions from ranked opportunities.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExecutionService {
    /// The scanner whose cost model and risk limits also govern the open.
    pub scanner: Scanner,
    /// Deterministic paper-fill parameters.
    pub fill: PaperFillConfig,
}

impl ExecutionService {
    /// Open a paper position from `route_id`, sizing each leg to `notional`
    /// (defaulting to the route's risk reservation, capped by capacity).
    ///
    /// # Errors
    /// Returns [`OpenError`] if the route is not in the latest scan, a leg has
    /// no executable fixture, or the fixtures fail to load.
    pub fn open(
        &self,
        route_id: &str,
        notional: Option<Decimal>,
        idempotency_key: String,
        now: DateTime<Utc>,
    ) -> Result<ArbitragePosition, OpenError> {
        self.open_with_scenario(
            route_id,
            notional,
            idempotency_key,
            PaperFillScenario::default(),
            now,
        )
    }

    /// Open with an explicit deterministic paper fill scenario, against the
    /// fixture snapshot for `now`.
    ///
    /// # Errors
    /// Returns [`OpenError`] as for [`Self::open_from`], or when the fixtures
    /// fail to load.
    pub fn open_with_scenario(
        &self,
        route_id: &str,
        notional: Option<Decimal>,
        idempotency_key: String,
        scenario: PaperFillScenario,
        now: DateTime<Utc>,
    ) -> Result<ArbitragePosition, OpenError> {
        let snapshot = fixture_snapshot(now).map_err(OpenError::Fixture)?;
        self.open_from(
            &snapshot,
            route_id,
            notional,
            idempotency_key,
            scenario,
            now,
        )
    }

    /// Open `route_id` against a market snapshot.
    ///
    /// The route is re-ranked from the snapshot's legs as of the snapshot time,
    /// so it matches the opportunity that was displayed. Each leg is sized to
    /// `notional` (defaulting to the route's risk reservation, capped by
    /// capacity) and fills from its captured book when present.
    ///
    /// # Errors
    /// Returns [`OpenError::RouteNotFound`] if the route is not eligible in the
    /// snapshot, [`OpenError::LegNotFound`] if a leg is missing, or
    /// [`OpenError::InsufficientDepth`] if a captured book cannot fill it.
    pub fn open_from(
        &self,
        snapshot: &MarketSnapshot,
        route_id: &str,
        notional: Option<Decimal>,
        idempotency_key: String,
        scenario: PaperFillScenario,
        now: DateTime<Utc>,
    ) -> Result<ArbitragePosition, OpenError> {
        let scan = self.scanner.scan_legs(snapshot.taken_at, &snapshot.legs);
        let ranked = scan
            .opportunities
            .into_iter()
            .find(|entry| entry.opportunity.route_id == route_id)
            .ok_or(OpenError::RouteNotFound)?;
        let opportunity = ranked.opportunity;
        let assessment = ranked.assessment;

        // Size to the reservation (or the caller's request), never above capacity.
        let cap = assessment
            .reserved_notional
            .min(opportunity.available_capacity);
        let requested = match notional {
            Some(value) if value > Decimal::ZERO => value.min(cap),
            _ => cap,
        };
        let reserved_capital = if opportunity.available_capacity.is_zero() {
            Decimal::ZERO
        } else {
            opportunity.required_capital * (requested / opportunity.available_capacity)
        };

        let long_exec = Self::leg_execution(
            snapshot,
            opportunity.route_type,
            opportunity.long_leg.forecast.venue,
            &opportunity.underlying,
            true,
            requested,
        )?;
        let short_exec = Self::leg_execution(
            snapshot,
            opportunity.route_type,
            opportunity.short_leg.forecast.venue,
            &opportunity.underlying,
            false,
            requested,
        )?;

        let id = Uuid::new_v4().to_string();
        Ok(open_paper_with_scenario(
            id,
            idempotency_key,
            &opportunity,
            &long_exec,
            &short_exec,
            requested,
            reserved_capital,
            &self.scanner.cost_model,
            &self.fill,
            &self.scanner.risk_limits,
            scenario,
            now,
        ))
    }

    fn leg_execution(
        snapshot: &MarketSnapshot,
        route_type: RouteType,
        venue: Venue,
        underlying: &str,
        is_long: bool,
        notional: Decimal,
    ) -> Result<LegExecution, OpenError> {
        // The long leg of a spot-perp route is spot; every other leg is a perp.
        let kind = if matches!((route_type, is_long), (RouteType::SpotPerp, true)) {
            InstrumentKind::Spot
        } else {
            InstrumentKind::Perp
        };
        let leg = snapshot
            .legs
            .iter()
            .find(|leg| {
                leg.kind == kind
                    && leg.observation.instrument.venue == venue
                    && leg.observation.instrument.underlying == underlying
            })
            .ok_or(OpenError::LegNotFound)?;
        // The scanner ranks degraded legs too; entries fail closed here so no
        // caller can open new exposure on a degraded feed.
        if leg.status != MarketStatus::Healthy {
            return Err(OpenError::LegDegraded);
        }
        let symbol = &leg.observation.instrument.symbol;
        let fill_price = match snapshot.book(venue, symbol) {
            Some(book) => Some(
                book_fill_price(book, is_long, notional, leg.observation.mark_price)
                    .ok_or(OpenError::InsufficientDepth)?,
            ),
            None => None,
        };
        Ok(LegExecution {
            symbol: symbol.clone(),
            mark_price: leg.observation.mark_price,
            funding_rate: leg.observation.funding_rate,
            funding_interval_hours: leg.observation.interval_hours,
            fill_price,
        })
    }

    /// Close an open paper position: capture funding, flatten both legs, and
    /// record realized PnL. A non-open position is returned unchanged.
    #[must_use]
    pub fn close(&self, position: ArbitragePosition, now: DateTime<Utc>) -> ArbitragePosition {
        close_paper(position, now, &self.scanner.cost_model, ExitReason::Manual)
    }

    /// Close with an engine-selected durable exit reason.
    #[must_use]
    pub fn close_with_reason(
        &self,
        position: ArbitragePosition,
        now: DateTime<Utc>,
        reason: ExitReason,
    ) -> ArbitragePosition {
        close_paper(position, now, &self.scanner.cost_model, reason)
    }

    /// Re-mark, re-hedge, and evaluate one existing paper position against
    /// the fixture snapshot for `now`.
    ///
    /// # Errors
    /// Returns [`SuperviseError`] as for [`Self::supervise_with`], or when the
    /// fixtures fail to load.
    pub fn supervise(
        &self,
        position: ArbitragePosition,
        now: DateTime<Utc>,
    ) -> Result<ArbitragePosition, SuperviseError> {
        let snapshot = fixture_snapshot(now).map_err(SuperviseError::Fixture)?;
        self.supervise_with(&snapshot, position, now)
    }

    /// Re-mark, re-hedge, and evaluate one existing paper position against a
    /// market snapshot.
    ///
    /// # Errors
    /// Returns [`SuperviseError`] when the snapshot cannot resolve both legs or
    /// the original route. The caller must retain the durable position
    /// unchanged and retry on the next tick.
    pub fn supervise_with(
        &self,
        snapshot: &MarketSnapshot,
        position: ArbitragePosition,
        now: DateTime<Utc>,
    ) -> Result<ArbitragePosition, SuperviseError> {
        if position.state.as_str() != "OPEN" {
            return Ok(position);
        }
        let legs = &snapshot.legs;
        let (long_mark, long_healthy) =
            Self::current_market(legs, position.long_leg.venue, &position.long_leg.symbol)?;
        let (short_mark, short_healthy) =
            Self::current_market(legs, position.short_leg.venue, &position.short_leg.symbol)?;
        let position = re_hedge(
            position,
            long_mark,
            short_mark,
            &self.scanner.risk_limits,
            &self.scanner.cost_model,
        );
        let opportunity = self
            .scanner
            .opportunity_in(&position.route_id, legs)
            .ok_or(SuperviseError::RouteNotFound)?;
        match evaluate_exit(
            opportunity.net_apr,
            long_healthy && short_healthy,
            &self.scanner.risk_limits,
        ) {
            ExitDecision::Hold => Ok(position),
            ExitDecision::Exit(reason) => {
                Ok(close_paper(position, now, &self.scanner.cost_model, reason))
            }
        }
    }

    fn current_market(
        legs: &[MarketLeg],
        venue: Venue,
        symbol: &str,
    ) -> Result<(Decimal, bool), SuperviseError> {
        legs.iter()
            .find(|leg| {
                leg.observation.instrument.venue == venue
                    && leg.observation.instrument.symbol == symbol
            })
            .map(|leg| {
                (
                    leg.observation.mark_price,
                    leg.status == MarketStatus::Healthy,
                )
            })
            .ok_or(SuperviseError::LegNotFound)
    }
}

/// The deterministic fixture snapshot for `now`: legs only, no books.
///
/// # Errors
/// Returns the fixture error if the embedded fixtures fail to load.
pub fn fixture_snapshot(now: DateTime<Utc>) -> Result<MarketSnapshot, fixtures::FixtureError> {
    Ok(MarketSnapshot::from_legs(now, fixtures::load_at(now)?))
}

/// The VWAP of filling `notional` (quote currency) against one side of a
/// captured book: asks for a buy, bids for a sell. Size is derived from the
/// leg's mark, as the domain sizes the order.
///
/// Returns `None` when the book is empty or too thin to fill the size.
#[must_use]
pub fn book_fill_price(
    book: &LegBook,
    is_buy: bool,
    notional: Decimal,
    mark_price: Decimal,
) -> Option<Decimal> {
    if mark_price <= Decimal::ZERO
        || notional <= Decimal::ZERO
        || book.contract_multiplier <= Decimal::ZERO
    {
        return None;
    }
    let side: &BookSide = if is_buy { &book.asks } else { &book.bids };
    let mut remaining = notional / mark_price; // base units
    let mut cost = Decimal::ZERO;
    let mut filled = Decimal::ZERO;
    for &(price, contracts) in &side.levels {
        if remaining <= Decimal::ZERO {
            break;
        }
        let available = contracts * book.contract_multiplier;
        let take = available.min(remaining);
        cost += take * price;
        filled += take;
        remaining -= take;
    }
    // Twelve places is far below any venue tick and drops division residue.
    (remaining <= Decimal::ZERO && !filled.is_zero()).then(|| (cost / filled).round_dp(12))
}

#[cfg(test)]
mod tests {
    use super::ExecutionService;
    use chrono::{TimeZone, Utc};
    use funding_arb_domain::position::{ExitReason, FillOutcome, PaperFillScenario};

    fn first_route_id(service: &ExecutionService, now: chrono::DateTime<Utc>) -> String {
        service
            .scanner
            .scan(now)
            .expect("scan")
            .opportunities
            .first()
            .expect("at least one opportunity")
            .opportunity
            .route_id
            .clone()
    }

    #[test]
    fn open_produces_an_open_position_for_a_ranked_route() {
        let service = ExecutionService::default();
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let route_id = first_route_id(&service, now);
        let position = service
            .open(&route_id, None, "key-open".into(), now)
            .expect("open succeeds");
        assert_eq!(position.route_id, route_id);
        assert_eq!(position.state.as_str(), "OPEN");
        assert!(position.long_leg.filled_qty > rust_decimal::Decimal::ZERO);
        assert!(position.short_leg.filled_qty > rust_decimal::Decimal::ZERO);
    }

    #[test]
    fn unknown_route_is_rejected() {
        let service = ExecutionService::default();
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let result = service.open("PERP_PERP:BINANCE>BYBIT:NOPE", None, "key".into(), now);
        assert!(matches!(result, Err(super::OpenError::RouteNotFound)));
    }

    #[test]
    fn partial_and_unknown_scenarios_reconcile_before_open() {
        let service = ExecutionService::default();
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let route_id = first_route_id(&service, now);
        let position = service
            .open_with_scenario(
                &route_id,
                None,
                "recover-service".into(),
                PaperFillScenario {
                    long: FillOutcome::Unknown(rust_decimal::Decimal::new(5, 1)),
                    short: FillOutcome::Partial(rust_decimal::Decimal::new(8, 1)),
                },
                now,
            )
            .expect("recovered open");
        assert_eq!(position.state.as_str(), "OPEN");
        assert!(position.long_leg.orders.len() > 1 || position.short_leg.orders.len() > 1);
    }

    #[test]
    fn close_captures_funding_and_flattens() {
        let service = ExecutionService::default();
        let opened = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let route_id = first_route_id(&service, opened);
        let position = service
            .open(&route_id, None, "key-close".into(), opened)
            .expect("open");
        assert_eq!(position.state.as_str(), "OPEN");

        let closed = service.close(position, opened + chrono::Duration::hours(24));
        assert_eq!(closed.state.as_str(), "CLOSED");
        assert!(closed.closed_at.is_some());
        assert!(closed.funding_captured > rust_decimal::Decimal::ZERO);
        assert_eq!(closed.long_leg.filled_qty, rust_decimal::Decimal::ZERO);
        assert_eq!(closed.exit_reason, Some(ExitReason::Manual));
    }

    #[test]
    fn supervise_rehedges_then_exits_across_the_market_path() {
        let service = ExecutionService::default();
        let t0 = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let t1 = Utc.with_ymd_and_hms(2026, 7, 19, 4, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 7, 19, 8, 0, 0).unwrap();
        let t3 = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        let position = service
            .open(
                "PERP_PERP:BINANCE>BYBIT:BTC",
                None,
                "key-supervise".into(),
                t0,
            )
            .expect("open");

        let at_t1 = service.supervise(position, t1).expect("t1 supervise");
        assert_eq!(at_t1.state.as_str(), "OPEN");
        assert_eq!(at_t1.residual_breach_ticks, 1);
        let at_t2 = service.supervise(at_t1, t2).expect("t2 supervise");
        assert_eq!(at_t2.state.as_str(), "OPEN");
        assert_eq!(at_t2.residual_breach_ticks, 0);
        assert!(
            at_t2
                .long_leg
                .orders
                .iter()
                .any(|order| order.client_order_id.contains("-hedge-long-")),
        );
        let repeated = service
            .supervise(at_t2.clone(), t2)
            .expect("repeat t2 supervise");
        assert_eq!(repeated.long_leg.orders.len(), at_t2.long_leg.orders.len());

        let at_t3 = service.supervise(at_t2, t3).expect("t3 supervise");
        assert_eq!(at_t3.state.as_str(), "CLOSED");
        assert_eq!(at_t3.exit_reason, Some(ExitReason::FundingFlip));
        assert_eq!(at_t3.long_leg.filled_qty, rust_decimal::Decimal::ZERO);
    }

    mod snapshot {
        use super::super::{OpenError, book_fill_price, fixture_snapshot};
        use super::{ExecutionService, first_route_id};
        use chrono::{TimeZone, Utc};
        use funding_arb_domain::position::PaperFillScenario;
        use funding_arb_scanner_service::fixtures::MarketStatus;
        use funding_arb_scanner_service::{BookSide, LegBook, MarketSnapshot};
        use rust_decimal::Decimal;

        fn dec(value: &str) -> Decimal {
            value.parse().expect("decimal")
        }

        fn book(levels_bid: &[(&str, &str)], levels_ask: &[(&str, &str)]) -> LegBook {
            let side = |levels: &[(&str, &str)]| BookSide {
                levels: levels.iter().map(|(p, q)| (dec(p), dec(q))).collect(),
            };
            LegBook {
                venue: funding_arb_domain::market::Venue::Binance,
                symbol: "X".into(),
                contract_multiplier: Decimal::ONE,
                bids: side(levels_bid),
                asks: side(levels_ask),
            }
        }

        /// The fixture snapshot with a deep, one-level book behind every leg,
        /// priced one tick through the mark on each side.
        fn booked(now: chrono::DateTime<Utc>, depth: &str) -> MarketSnapshot {
            let mut snapshot = fixture_snapshot(now).expect("fixtures");
            snapshot.books = snapshot
                .legs
                .iter()
                .map(|leg| {
                    let mark = leg.observation.mark_price;
                    LegBook {
                        venue: leg.observation.instrument.venue,
                        symbol: leg.observation.instrument.symbol.clone(),
                        contract_multiplier: Decimal::ONE,
                        bids: BookSide {
                            levels: vec![(mark - Decimal::ONE, dec(depth))],
                        },
                        asks: BookSide {
                            levels: vec![(mark + Decimal::ONE, dec(depth))],
                        },
                    }
                })
                .collect();
            snapshot
        }

        #[test]
        fn vwap_walks_levels_best_first() {
            let book = book(&[("99", "1"), ("98", "10")], &[("101", "1"), ("103", "10")]);
            // Buy 3 base at mark 100: 1 @ 101 + 2 @ 103 = 307 / 3.
            assert_eq!(
                book_fill_price(&book, true, dec("300"), dec("100")),
                Some((dec("307") / dec("3")).round_dp(12))
            );
            // Sell 2 base: 1 @ 99 + 1 @ 98.
            assert_eq!(
                book_fill_price(&book, false, dec("200"), dec("100")),
                Some(dec("98.5"))
            );
        }

        #[test]
        fn vwap_applies_the_contract_multiplier() {
            let mut book = book(&[], &[("100", "50")]);
            book.contract_multiplier = dec("0.01"); // 50 contracts = 0.5 base
            assert_eq!(
                book_fill_price(&book, true, dec("40"), dec("100")),
                Some(dec("100"))
            );
            assert_eq!(book_fill_price(&book, true, dec("60"), dec("100")), None);
        }

        #[test]
        fn vwap_rejects_thin_empty_or_degenerate_input() {
            let thin = book(&[("99", "1")], &[("101", "1")]);
            assert_eq!(book_fill_price(&thin, true, dec("1000"), dec("100")), None);
            assert_eq!(
                book_fill_price(&book(&[], &[]), true, dec("1"), dec("100")),
                None
            );
            assert_eq!(
                book_fill_price(&thin, true, Decimal::ZERO, dec("100")),
                None
            );
            assert_eq!(book_fill_price(&thin, true, dec("1"), Decimal::ZERO), None);
        }

        #[test]
        fn a_booked_snapshot_fills_each_leg_from_its_book() {
            let service = ExecutionService::default();
            let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
            let route_id = first_route_id(&service, now);
            let snapshot = booked(now, "1000000");
            let position = service
                .open_from(
                    &snapshot,
                    &route_id,
                    None,
                    "key".into(),
                    PaperFillScenario::default(),
                    now,
                )
                .expect("open from snapshot");
            // Buy crosses one above the mark, sell one below.
            assert_eq!(
                position.long_leg.fills[0].price,
                position.long_leg.mark_price + Decimal::ONE
            );
            assert_eq!(
                position.short_leg.fills[0].price,
                position.short_leg.mark_price - Decimal::ONE
            );
        }

        #[test]
        fn a_thin_book_refuses_the_open() {
            let service = ExecutionService::default();
            let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
            let route_id = first_route_id(&service, now);
            let result = service.open_from(
                &booked(now, "0.000001"),
                &route_id,
                None,
                "key".into(),
                PaperFillScenario::default(),
                now,
            );
            assert!(matches!(result, Err(OpenError::InsufficientDepth)));
        }

        #[test]
        fn a_fixture_snapshot_opens_exactly_like_the_legacy_path() {
            let service = ExecutionService::default();
            let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
            let route_id = first_route_id(&service, now);
            let legacy = service
                .open(&route_id, None, "key".into(), now)
                .expect("legacy");
            let snapshot = service
                .open_from(
                    &fixture_snapshot(now).expect("fixtures"),
                    &route_id,
                    None,
                    "key".into(),
                    PaperFillScenario::default(),
                    now,
                )
                .expect("snapshot");
            let fills = |leg: &funding_arb_domain::position::PositionLeg| {
                leg.fills
                    .iter()
                    .map(|fill| (fill.qty, fill.price, fill.fee))
                    .collect::<Vec<_>>()
            };
            assert_eq!(fills(&legacy.long_leg), fills(&snapshot.long_leg));
            assert_eq!(fills(&legacy.short_leg), fills(&snapshot.short_leg));
            assert_eq!(legacy.state, snapshot.state);
        }

        #[test]
        fn a_degraded_leg_refuses_new_exposure() {
            let service = ExecutionService::default();
            let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
            let route_id = first_route_id(&service, now);
            let mut snapshot = fixture_snapshot(now).expect("fixtures");
            for leg in &mut snapshot.legs {
                leg.status = MarketStatus::Degraded;
            }
            let result = service.open_from(
                &snapshot,
                &route_id,
                None,
                "key".into(),
                PaperFillScenario::default(),
                now,
            );
            assert!(matches!(result, Err(OpenError::LegDegraded)));
        }

        #[test]
        fn supervision_uses_the_snapshot_and_fails_safe_when_a_leg_vanishes() {
            let service = ExecutionService::default();
            let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
            let route_id = first_route_id(&service, now);
            let snapshot = fixture_snapshot(now).expect("fixtures");
            let position = service
                .open_from(
                    &snapshot,
                    &route_id,
                    None,
                    "key".into(),
                    PaperFillScenario::default(),
                    now,
                )
                .expect("open");
            let held = service
                .supervise_with(&snapshot, position.clone(), now)
                .expect("supervise");
            assert_eq!(held.state.as_str(), "OPEN");
            let empty = MarketSnapshot::from_legs(now, Vec::new());
            assert!(service.supervise_with(&empty, position, now).is_err());
        }
    }
}
