//! Opening a paper position from a ranked opportunity.
//!
//! Re-runs the deterministic `Scanner` to locate `route_id`, resolves each
//! leg's executable mark price from the same fixtures the scan ranked, then runs
//! the domain `open_paper` simulator. Because both the scan and the fill are
//! pure functions of the fixtures, opening is fully reproducible.

use std::fmt;

use chrono::{DateTime, Utc};
use funding_arb_domain::market::{RouteType, Venue};
use funding_arb_domain::position::{
    ArbitragePosition, ExitDecision, ExitReason, LegExecution, PaperFillConfig, PaperFillScenario,
    close_paper, evaluate_exit, open_paper_with_scenario, re_hedge,
};
use funding_arb_scanner_service::Scanner;
use funding_arb_scanner_service::fixtures::{self, InstrumentKind, MarketLeg, MarketStatus};
use rust_decimal::Decimal;
use uuid::Uuid;

/// Why a paper open could not be produced.
#[derive(Debug)]
pub enum OpenError {
    /// No opportunity with the requested `route_id` is in the latest scan.
    RouteNotFound,
    /// A leg of the matched route has no executable fixture leg.
    LegNotFound,
    /// The market fixtures could not be loaded.
    Fixture(fixtures::FixtureError),
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RouteNotFound => formatter.write_str("route not found in the latest scan"),
            Self::LegNotFound => formatter.write_str("executable leg not found for the route"),
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

    /// Open with an explicit deterministic paper fill scenario.
    pub fn open_with_scenario(
        &self,
        route_id: &str,
        notional: Option<Decimal>,
        idempotency_key: String,
        scenario: PaperFillScenario,
        now: DateTime<Utc>,
    ) -> Result<ArbitragePosition, OpenError> {
        let scan = self.scanner.scan(now).map_err(OpenError::Fixture)?;
        let ranked = scan
            .opportunities
            .into_iter()
            .find(|entry| entry.opportunity.route_id == route_id)
            .ok_or(OpenError::RouteNotFound)?;
        let opportunity = ranked.opportunity;
        let assessment = ranked.assessment;

        let legs = fixtures::load_at(now).map_err(OpenError::Fixture)?;
        let long_exec = Self::leg_execution(
            &legs,
            opportunity.route_type,
            opportunity.long_leg.forecast.venue,
            &opportunity.underlying,
            true,
        )?;
        let short_exec = Self::leg_execution(
            &legs,
            opportunity.route_type,
            opportunity.short_leg.forecast.venue,
            &opportunity.underlying,
            false,
        )?;

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
        legs: &[MarketLeg],
        route_type: RouteType,
        venue: Venue,
        underlying: &str,
        is_long: bool,
    ) -> Result<LegExecution, OpenError> {
        // The long leg of a spot-perp route is spot; every other leg is a perp.
        let kind = if matches!((route_type, is_long), (RouteType::SpotPerp, true)) {
            InstrumentKind::Spot
        } else {
            InstrumentKind::Perp
        };
        legs.iter()
            .find(|leg| {
                leg.kind == kind
                    && leg.observation.instrument.venue == venue
                    && leg.observation.instrument.underlying == underlying
            })
            .map(|leg| LegExecution {
                symbol: leg.observation.instrument.symbol.clone(),
                mark_price: leg.observation.mark_price,
                funding_rate: leg.observation.funding_rate,
                funding_interval_hours: leg.observation.interval_hours,
            })
            .ok_or(OpenError::LegNotFound)
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

    /// Re-mark, re-hedge, and evaluate one existing paper position at `now`.
    ///
    /// # Errors
    /// Returns [`SuperviseError`] when current fixtures cannot resolve both
    /// legs or the original route. The caller must retain the durable position
    /// unchanged and retry on the next tick.
    pub fn supervise(
        &self,
        position: ArbitragePosition,
        now: DateTime<Utc>,
    ) -> Result<ArbitragePosition, SuperviseError> {
        if position.state.as_str() != "OPEN" {
            return Ok(position);
        }
        let legs = fixtures::load_at(now).map_err(SuperviseError::Fixture)?;
        let (long_mark, long_healthy) =
            Self::current_market(&legs, position.long_leg.venue, &position.long_leg.symbol)?;
        let (short_mark, short_healthy) =
            Self::current_market(&legs, position.short_leg.venue, &position.short_leg.symbol)?;
        let position = re_hedge(
            position,
            long_mark,
            short_mark,
            &self.scanner.risk_limits,
            &self.scanner.cost_model,
        );
        let opportunity = self
            .scanner
            .opportunity_at(&position.route_id, now)
            .map_err(SuperviseError::Fixture)?
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
}
