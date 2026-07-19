//! Opportunity construction, costing, and ranking.
//!
//! A route pairs two legs on one underlying. By convention the **short leg**
//! earns positive funding and the **long leg** pays it, so the gross route
//! funding APR is `short.predicted - long.predicted`. Costs are annualized over
//! the configured holding period and subtracted to yield `net_apr`. Ranking and
//! eligibility follow PRODUCT_SPEC §4.5.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::forecast::FundingForecast;
use crate::market::RouteType;

fn abs(value: Decimal) -> Decimal {
    if value.is_sign_negative() { -value } else { value }
}

fn clamp_unit(value: Decimal) -> Decimal {
    value.clamp(Decimal::ZERO, Decimal::ONE)
}

/// One leg of a two-leg route.
#[derive(Clone, Debug)]
pub struct RouteLeg {
    /// Whether this leg is held long (pays positive funding) or short (earns it).
    pub is_long: bool,
    /// The forecast that drives this leg's contribution.
    pub forecast: FundingForecast,
}

/// Cost and capacity assumptions used to cost a route (safe paper defaults).
#[derive(Clone, Copy, Debug)]
pub struct CostModel {
    /// Taker fee per side, as a fraction of notional.
    pub taker_fee: Decimal,
    /// Slippage per side, as a fraction of notional.
    pub slippage: Decimal,
    /// Borrow/carry drag, expressed directly as an APR fraction.
    pub carry_ratio: Decimal,
    /// Risk buffer as a fraction of the gross funding APR.
    pub risk_buffer_ratio: Decimal,
    /// Assumed holding period, in days, used to annualize one-off costs.
    pub holding_days: Decimal,
    /// Minimum executable notional (USD) for a route to be eligible.
    pub min_required_notional: Decimal,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            taker_fee: Decimal::new(5, 4),          // 0.0005 (5 bps)
            slippage: Decimal::new(3, 4),           // 0.0003 (3 bps)
            carry_ratio: Decimal::new(2, 2),        // 0.02 (2% APR)
            risk_buffer_ratio: Decimal::new(10, 2), // 0.10 of gross
            holding_days: Decimal::from(14),
            min_required_notional: Decimal::from(1000),
        }
    }
}

/// A ranked, costed funding-arbitrage opportunity over a two-leg route.
#[derive(Clone, Debug)]
pub struct Opportunity {
    /// A stable identifier for the route (`ROUTE_TYPE:long>short:underlying`).
    pub route_id: String,
    /// Whether the route is spot-perp or perp-perp.
    pub route_type: RouteType,
    /// The underlying asset the route trades.
    pub underlying: String,
    /// The leg held long.
    pub long_leg: RouteLeg,
    /// The leg held short.
    pub short_leg: RouteLeg,
    /// Gross funding APR before costs.
    pub gross_funding_apr: Decimal,
    /// Annualized entry cost.
    pub entry_cost: Decimal,
    /// Annualized exit cost.
    pub exit_cost: Decimal,
    /// Annualized carry cost.
    pub carry_cost: Decimal,
    /// Risk buffer deducted from the APR.
    pub risk_buffer: Decimal,
    /// Net APR after all costs.
    pub net_apr: Decimal,
    /// Expected net PnL (USD) over the holding period at full capacity.
    pub expected_net_pnl: Decimal,
    /// Estimated probability that net PnL is positive, in `[0, 1]`.
    pub profitability_probability: Decimal,
    /// Depth-limited executable capacity (USD).
    pub available_capacity: Decimal,
    /// When funding next settles (earliest of the two legs).
    pub next_funding_at: DateTime<Utc>,
}

impl Opportunity {
    /// Build and cost an opportunity from its two legs and executable capacity.
    #[must_use]
    pub fn build(
        route_type: RouteType,
        underlying: &str,
        long_leg: RouteLeg,
        short_leg: RouteLeg,
        available_capacity: Decimal,
        cost: &CostModel,
    ) -> Self {
        let gross =
            short_leg.forecast.predicted_settlement_rate - long_leg.forecast.predicted_settlement_rate;

        let annualization = if cost.holding_days.is_zero() {
            Decimal::ZERO
        } else {
            Decimal::from(365) / cost.holding_days
        };
        let per_side = cost.taker_fee + cost.slippage;
        // Two legs entered and two legs exited: 2 sides on entry, 2 on exit.
        let entry_cost = per_side * Decimal::from(2) * annualization;
        let exit_cost = per_side * Decimal::from(2) * annualization;
        let carry_cost = cost.carry_ratio;
        let risk_buffer = abs(gross) * cost.risk_buffer_ratio;

        let net_apr = gross - entry_cost - exit_cost - carry_cost - risk_buffer;
        let expected_net_pnl = available_capacity * net_apr * (cost.holding_days / Decimal::from(365));

        let confidence = (long_leg.forecast.direction_probability
            + short_leg.forecast.direction_probability)
            / Decimal::from(2);
        let margin_base = abs(gross) + risk_buffer;
        let margin = if margin_base.is_zero() {
            Decimal::ZERO
        } else {
            net_apr / margin_base
        };
        let profitability_probability =
            clamp_unit(confidence * (Decimal::new(5, 1) + Decimal::new(5, 1) * margin));

        let next_funding_at = long_leg
            .forecast
            .next_funding_at
            .min(short_leg.forecast.next_funding_at);
        let route_id = format!(
            "{}:{}>{}:{}",
            route_type.as_str(),
            long_leg.forecast.venue.as_str(),
            short_leg.forecast.venue.as_str(),
            underlying,
        );

        Self {
            route_id,
            route_type,
            underlying: underlying.to_owned(),
            long_leg,
            short_leg,
            gross_funding_apr: gross,
            entry_cost,
            exit_cost,
            carry_cost,
            risk_buffer,
            net_apr,
            expected_net_pnl,
            profitability_probability,
            available_capacity,
            next_funding_at,
        }
    }

    /// Whether the route clears the eligibility gates (positive net APR and
    /// enough executable capacity).
    #[must_use]
    pub fn is_eligible(&self, cost: &CostModel) -> bool {
        self.net_apr > Decimal::ZERO && self.available_capacity >= cost.min_required_notional
    }
}

/// Drop ineligible routes and rank the rest by the PRODUCT_SPEC §4.5 key:
/// `net_apr ↓, profitability_probability ↓, available_capacity ↓,
/// next_funding_at ↑, route_id ↑`.
#[must_use]
pub fn rank(mut opportunities: Vec<Opportunity>, cost: &CostModel) -> Vec<Opportunity> {
    opportunities.retain(|opportunity| opportunity.is_eligible(cost));
    opportunities.sort_by(|a, b| {
        b.net_apr
            .cmp(&a.net_apr)
            .then_with(|| b.profitability_probability.cmp(&a.profitability_probability))
            .then_with(|| b.available_capacity.cmp(&a.available_capacity))
            .then_with(|| a.next_funding_at.cmp(&b.next_funding_at))
            .then_with(|| a.route_id.cmp(&b.route_id))
    });
    opportunities
}

#[cfg(test)]
mod tests {
    use super::{CostModel, Opportunity, RouteLeg, rank};
    use crate::forecast::{ForecastConfig, forecast};
    use crate::market::{FundingObservation, InstrumentRef, RouteType, Venue};
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    fn observation(
        venue: Venue,
        underlying: &str,
        rate: Decimal,
        mark: Decimal,
        index: Decimal,
        capacity: Decimal,
    ) -> FundingObservation {
        FundingObservation {
            instrument: InstrumentRef {
                venue,
                symbol: format!("{underlying}USDT"),
                underlying: underlying.to_owned(),
            },
            funding_rate: rate,
            interval_hours: 8,
            mark_price: mark,
            index_price: index,
            available_notional: capacity,
            observed_at: Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap(),
            next_funding_at: Utc.with_ymd_and_hms(2026, 7, 19, 8, 0, 0).unwrap(),
        }
    }

    fn opportunity(long_rate: Decimal, short_rate: Decimal, capacity: Decimal) -> Opportunity {
        let config = ForecastConfig::default();
        let long = observation(
            Venue::Binance,
            "BTC",
            long_rate,
            Decimal::from(30_000),
            Decimal::from(30_000),
            capacity,
        );
        let short = observation(
            Venue::Bybit,
            "BTC",
            short_rate,
            Decimal::from(30_030),
            Decimal::from(30_000),
            capacity,
        );
        Opportunity::build(
            RouteType::PerpPerp,
            "BTC",
            RouteLeg { is_long: true, forecast: forecast(&long, &config) },
            RouteLeg { is_long: false, forecast: forecast(&short, &config) },
            capacity,
            &CostModel::default(),
        )
    }

    #[test]
    fn forecast_is_deterministic() {
        let config = ForecastConfig::default();
        let obs = observation(
            Venue::Binance,
            "BTC",
            Decimal::new(1, 4),
            Decimal::from(30_030),
            Decimal::from(30_000),
            Decimal::from(50_000),
        );
        let a = forecast(&obs, &config);
        let b = forecast(&obs, &config);
        assert_eq!(a.predicted_settlement_rate, b.predicted_settlement_rate);
        assert_eq!(a.p10_rate, b.p10_rate);
        assert_eq!(a.p90_rate, b.p90_rate);
        assert!(a.p10_rate < a.p90_rate);
    }

    #[test]
    fn ranking_orders_by_net_apr_and_drops_ineligible() {
        let cost = CostModel::default();
        // A wide positive spread, a thin one, and a negative (ineligible) one.
        let strong = opportunity(Decimal::new(1, 4), Decimal::new(30, 4), Decimal::from(100_000));
        let weak = opportunity(Decimal::new(1, 4), Decimal::new(9, 4), Decimal::from(100_000));
        let losing = opportunity(Decimal::new(30, 4), Decimal::new(1, 4), Decimal::from(100_000));
        assert!(strong.net_apr > weak.net_apr);
        assert!(!losing.is_eligible(&cost));

        let ranked = rank(vec![weak.clone(), losing, strong.clone()], &cost);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].route_id, strong.route_id);
        assert!(ranked[0].net_apr >= ranked[1].net_apr);
    }

    #[test]
    fn capacity_gate_excludes_thin_routes() {
        let cost = CostModel::default();
        let thin = opportunity(Decimal::new(1, 4), Decimal::new(30, 4), Decimal::from(500));
        assert!(!thin.is_eligible(&cost));
        assert!(rank(vec![thin], &cost).is_empty());
    }
}
