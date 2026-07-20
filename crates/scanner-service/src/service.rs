//! The deterministic funding-arbitrage scanner.
//!
//! Loads fixture markets, forecasts each leg, enumerates two-leg routes per
//! underlying (perp-perp across venue pairs and spot-perp), costs, gates them
//! against the risk limits, ranks, and attaches a per-opportunity risk
//! reservation. Pure and reproducible — the only non-determinism is the
//! `scanned_at` stamp the caller supplies.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use funding_arb_domain::forecast::{ForecastConfig, MODEL_VERSION, forecast};
use funding_arb_domain::market::RouteType;
use funding_arb_domain::opportunity::{CostModel, Opportunity, RouteLeg, rank};
use funding_arb_domain::risk::{RiskAssessment, RiskLimits};

use crate::fixtures::{self, FixtureError, InstrumentKind, MarketLeg};

/// A ranked opportunity with its risk reservation.
#[derive(Clone, Debug)]
pub struct RankedOpportunity {
    /// The costed opportunity.
    pub opportunity: Opportunity,
    /// The paper risk reservation and limit status.
    pub assessment: RiskAssessment,
}

/// The output of one scan: a ranked opportunity set with provenance.
#[derive(Clone, Debug)]
pub struct ScanResult {
    /// When the scan was produced.
    pub scanned_at: DateTime<Utc>,
    /// The forecasting model version used.
    pub model_version: &'static str,
    /// Eligible opportunities in ranked order, with risk assessments.
    pub opportunities: Vec<RankedOpportunity>,
}

/// The deterministic scanner: fixtures → forecasts → risk-gated ranked opportunities.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scanner {
    /// Forecasting parameters.
    pub forecast_config: ForecastConfig,
    /// Cost parameters.
    pub cost_model: CostModel,
    /// Risk limits (eligibility gates + reservation sizing).
    pub risk_limits: RiskLimits,
}

impl Scanner {
    /// Run a full scan, stamping the result with `now`.
    ///
    /// # Errors
    /// Returns [`FixtureError`] if the embedded market fixtures cannot load.
    pub fn scan(&self, now: DateTime<Utc>) -> Result<ScanResult, FixtureError> {
        let legs = fixtures::load_at(now)?;
        let ranked = rank(self.build_opportunities(&legs), &self.risk_limits);
        let opportunities = ranked
            .into_iter()
            .map(|opportunity| {
                let assessment = self.risk_limits.assess(&opportunity);
                RankedOpportunity {
                    opportunity,
                    assessment,
                }
            })
            .collect();
        Ok(ScanResult {
            scanned_at: now,
            model_version: MODEL_VERSION,
            opportunities,
        })
    }

    /// Rebuild one route at `now` before risk/eligibility filtering.
    ///
    /// Position supervision must continue evaluating an existing route after a
    /// funding flip makes it disappear from the eligible scanner output.
    ///
    /// # Errors
    /// Returns [`FixtureError`] if the embedded market path cannot load.
    pub fn opportunity_at(
        &self,
        route_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Opportunity>, FixtureError> {
        let legs = fixtures::load_at(now)?;
        Ok(self
            .build_opportunities(&legs)
            .into_iter()
            .find(|opportunity| opportunity.route_id == route_id))
    }

    fn build_opportunities(&self, legs: &[MarketLeg]) -> Vec<Opportunity> {
        let mut opportunities = Vec::new();
        let underlyings: BTreeSet<&str> = legs
            .iter()
            .map(|leg| leg.observation.instrument.underlying.as_str())
            .collect();

        for underlying in underlyings {
            let group: Vec<&MarketLeg> = legs
                .iter()
                .filter(|leg| leg.observation.instrument.underlying == underlying)
                .collect();
            let perps: Vec<&MarketLeg> = group
                .iter()
                .copied()
                .filter(|leg| leg.kind == InstrumentKind::Perp)
                .collect();
            let spots: Vec<&MarketLeg> = group
                .iter()
                .copied()
                .filter(|leg| leg.kind == InstrumentKind::Spot)
                .collect();

            // Perp-perp: both orientations across distinct venues (one side of
            // each pair is filtered out later for negative net APR).
            for &long in &perps {
                for &short in &perps {
                    if long.observation.instrument.venue == short.observation.instrument.venue {
                        continue;
                    }
                    opportunities.push(self.route(RouteType::PerpPerp, underlying, long, short));
                }
            }
            // Spot-perp: long spot, short perp.
            for &long in &spots {
                for &short in &perps {
                    opportunities.push(self.route(RouteType::SpotPerp, underlying, long, short));
                }
            }
        }
        opportunities
    }

    fn route(
        &self,
        route_type: RouteType,
        underlying: &str,
        long: &MarketLeg,
        short: &MarketLeg,
    ) -> Opportunity {
        let long_forecast = forecast(&long.observation, &self.forecast_config);
        let short_forecast = forecast(&short.observation, &self.forecast_config);
        let capacity = long
            .observation
            .available_notional
            .min(short.observation.available_notional);
        Opportunity::build(
            route_type,
            underlying,
            RouteLeg {
                is_long: true,
                forecast: long_forecast,
            },
            RouteLeg {
                is_long: false,
                forecast: short_forecast,
            },
            capacity,
            &self.cost_model,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Scanner;
    use chrono::{TimeZone, Utc};

    #[test]
    fn scan_is_deterministic_ranked_and_within_limits() {
        let scanner = Scanner::default();
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let first = scanner.scan(now).expect("fixtures load");
        let second = scanner.scan(now).expect("fixtures load");

        assert!(!first.opportunities.is_empty());
        let ids: Vec<&str> = first
            .opportunities
            .iter()
            .map(|o| o.opportunity.route_id.as_str())
            .collect();
        let ids2: Vec<&str> = second
            .opportunities
            .iter()
            .map(|o| o.opportunity.route_id.as_str())
            .collect();
        assert_eq!(ids, ids2, "scan output must be deterministic");

        for pair in first.opportunities.windows(2) {
            assert!(
                pair[0].opportunity.net_apr >= pair[1].opportunity.net_apr,
                "opportunities must be net-APR ranked",
            );
        }
        // Every ranked opportunity clears the hard risk gates and carries a reservation.
        assert!(
            first
                .opportunities
                .iter()
                .all(|o| o.assessment.within_limits)
        );
        assert!(
            first
                .opportunities
                .iter()
                .all(|o| o.assessment.reserved_notional > rust_decimal::Decimal::ZERO)
        );
    }

    #[test]
    fn route_remains_evaluable_after_it_is_filtered_from_the_scan() {
        let scanner = Scanner::default();
        let route_id = "PERP_PERP:BINANCE>BYBIT:BTC";
        let t3 = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        assert!(
            scanner
                .scan(t3)
                .expect("scan")
                .opportunities
                .iter()
                .all(|ranked| ranked.opportunity.route_id != route_id),
        );
        let route = scanner
            .opportunity_at(route_id, t3)
            .expect("fixtures")
            .expect("route");
        assert!(route.net_apr <= rust_decimal::Decimal::ZERO);
    }
}
