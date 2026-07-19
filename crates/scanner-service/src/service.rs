//! The deterministic funding-arbitrage scanner.
//!
//! Loads fixture markets, forecasts each leg, enumerates two-leg routes per
//! underlying (perp-perp across venue pairs and spot-perp), costs and ranks
//! them. Pure and reproducible — the only non-determinism is the `scanned_at`
//! stamp the caller supplies.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use funding_arb_domain::forecast::{ForecastConfig, MODEL_VERSION, forecast};
use funding_arb_domain::market::RouteType;
use funding_arb_domain::opportunity::{CostModel, Opportunity, RouteLeg, rank};

use crate::fixtures::{self, FixtureError, InstrumentKind, MarketLeg};

/// The output of one scan: a ranked opportunity set with provenance.
#[derive(Clone, Debug)]
pub struct ScanResult {
    /// When the scan was produced.
    pub scanned_at: DateTime<Utc>,
    /// The forecasting model version used.
    pub model_version: &'static str,
    /// Eligible opportunities in ranked order.
    pub opportunities: Vec<Opportunity>,
}

/// The deterministic scanner: fixtures → forecasts → ranked opportunities.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scanner {
    /// Forecasting parameters.
    pub forecast_config: ForecastConfig,
    /// Cost and eligibility parameters.
    pub cost_model: CostModel,
}

impl Scanner {
    /// Run a full scan, stamping the result with `now`.
    ///
    /// # Errors
    /// Returns [`FixtureError`] if the embedded market fixtures cannot load.
    pub fn scan(&self, now: DateTime<Utc>) -> Result<ScanResult, FixtureError> {
        let legs = fixtures::load()?;
        let opportunities = rank(self.build_opportunities(&legs), &self.cost_model);
        Ok(ScanResult {
            scanned_at: now,
            model_version: MODEL_VERSION,
            opportunities,
        })
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
            RouteLeg { is_long: true, forecast: long_forecast },
            RouteLeg { is_long: false, forecast: short_forecast },
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
    fn scan_is_deterministic_and_ranked() {
        let scanner = Scanner::default();
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let first = scanner.scan(now).expect("fixtures load");
        let second = scanner.scan(now).expect("fixtures load");

        assert!(!first.opportunities.is_empty());
        let ids: Vec<&str> = first.opportunities.iter().map(|o| o.route_id.as_str()).collect();
        let ids2: Vec<&str> = second.opportunities.iter().map(|o| o.route_id.as_str()).collect();
        assert_eq!(ids, ids2, "scan output must be deterministic");

        for pair in first.opportunities.windows(2) {
            assert!(pair[0].net_apr >= pair[1].net_apr, "opportunities must be net-APR ranked");
        }
        assert!(first.opportunities.iter().all(|o| o.net_apr > rust_decimal::Decimal::ZERO));
    }
}
