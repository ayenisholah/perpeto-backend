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
    ArbitragePosition, LegExecution, PaperFillConfig, close_paper, open_paper,
};
use funding_arb_scanner_service::Scanner;
use funding_arb_scanner_service::fixtures::{self, InstrumentKind, MarketLeg};
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
        let scan = self.scanner.scan(now).map_err(OpenError::Fixture)?;
        let ranked = scan
            .opportunities
            .into_iter()
            .find(|entry| entry.opportunity.route_id == route_id)
            .ok_or(OpenError::RouteNotFound)?;
        let opportunity = ranked.opportunity;
        let assessment = ranked.assessment;

        let legs = fixtures::load().map_err(OpenError::Fixture)?;
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
        Ok(open_paper(
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
        close_paper(position, now, &self.scanner.cost_model)
    }
}

#[cfg(test)]
mod tests {
    use super::ExecutionService;
    use chrono::{TimeZone, Utc};

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
    }
}
