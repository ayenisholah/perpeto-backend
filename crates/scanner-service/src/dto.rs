//! Wire DTOs for the opportunity scan (numbers rendered as plain JSON floats).
//!
//! `OpportunityDto` is used for both the list and the detail view, and is also
//! the serialized payload persisted per opportunity, so it derives `Deserialize`.

use funding_arb_domain::opportunity::Opportunity;
use funding_arb_domain::risk::{RiskAssessment, RiskLimits};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

fn as_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

/// A ranked scan with provenance, as served by the API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpportunityScanDto {
    /// RFC 3339 timestamp of when the scan was produced.
    pub scanned_at: String,
    /// The forecasting model version.
    pub model_version: String,
    /// Ranked, eligible opportunities.
    pub opportunities: Vec<OpportunityDto>,
}

/// Per-leg forecast summary for the detail view.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegDto {
    /// The leg's venue.
    pub venue: String,
    /// Annualized predicted settlement rate.
    pub predicted_rate: f64,
    /// Lower (p10) annualized-rate bound.
    pub p10_rate: f64,
    /// Upper (p90) annualized-rate bound.
    pub p90_rate: f64,
    /// Probability the rate keeps its sign.
    pub direction_probability: f64,
    /// RFC 3339 timestamp of the leg's next funding settlement.
    pub next_funding_at: String,
}

/// A single costed, ranked opportunity with its risk reservation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpportunityDto {
    /// Stable route identifier.
    pub route_id: String,
    /// `SPOT_PERP` or `PERP_PERP`.
    pub route_type: String,
    /// The underlying asset.
    pub underlying: String,
    /// `POSITIVE` or `NEGATIVE` funding capture.
    pub funding_direction: String,
    /// The venue held long.
    pub long_venue: String,
    /// The venue held short.
    pub short_venue: String,
    /// Gross funding APR before costs.
    pub gross_funding_apr: f64,
    /// Annualized entry cost.
    pub entry_cost: f64,
    /// Annualized exit cost.
    pub exit_cost: f64,
    /// Annualized carry cost.
    pub carry_cost: f64,
    /// Risk buffer deducted from APR.
    pub risk_buffer: f64,
    /// Gross funding APR required to break even.
    pub break_even_funding: f64,
    /// Net APR after costs.
    pub net_apr: f64,
    /// Expected net PnL (USD) over the holding period.
    pub expected_net_pnl: f64,
    /// Probability net PnL is positive, in `[0, 1]`.
    pub profitability_probability: f64,
    /// Entry basis between legs, in basis points.
    pub entry_basis_bps: f64,
    /// Assumed leverage on each perpetual leg.
    pub leverage: f64,
    /// Capital (USD) tied up at full capacity.
    pub required_capital: f64,
    /// Depth-limited executable capacity (USD).
    pub available_capacity: f64,
    /// Notional (USD) reserved under the per-opportunity cap.
    pub reserved_notional: f64,
    /// Capital (USD) reserved.
    pub reserved_capital: f64,
    /// Reserved capital as a fraction of reference equity.
    pub capital_fraction: f64,
    /// Whether the opportunity clears every hard risk gate.
    pub within_limits: bool,
    /// The long leg's forecast summary.
    pub long_leg: LegDto,
    /// The short leg's forecast summary.
    pub short_leg: LegDto,
    /// RFC 3339 timestamp of the next funding settlement.
    pub next_funding_at: String,
}

impl LegDto {
    fn from_domain(leg: &funding_arb_domain::opportunity::RouteLeg) -> Self {
        Self {
            venue: leg.forecast.venue.as_str().to_owned(),
            predicted_rate: as_f64(leg.forecast.predicted_settlement_rate),
            p10_rate: as_f64(leg.forecast.p10_rate),
            p90_rate: as_f64(leg.forecast.p90_rate),
            direction_probability: as_f64(leg.forecast.direction_probability),
            next_funding_at: leg.forecast.next_funding_at.to_rfc3339(),
        }
    }
}

impl OpportunityDto {
    /// Project a domain opportunity and its risk assessment onto the wire shape.
    #[must_use]
    pub fn from_domain(opportunity: &Opportunity, assessment: &RiskAssessment) -> Self {
        Self {
            route_id: opportunity.route_id.clone(),
            route_type: opportunity.route_type.as_str().to_owned(),
            underlying: opportunity.underlying.clone(),
            funding_direction: opportunity.funding_direction.as_str().to_owned(),
            long_venue: opportunity.long_leg.forecast.venue.as_str().to_owned(),
            short_venue: opportunity.short_leg.forecast.venue.as_str().to_owned(),
            gross_funding_apr: as_f64(opportunity.gross_funding_apr),
            entry_cost: as_f64(opportunity.entry_cost),
            exit_cost: as_f64(opportunity.exit_cost),
            carry_cost: as_f64(opportunity.carry_cost),
            risk_buffer: as_f64(opportunity.risk_buffer),
            break_even_funding: as_f64(opportunity.break_even_funding),
            net_apr: as_f64(opportunity.net_apr),
            expected_net_pnl: as_f64(opportunity.expected_net_pnl),
            profitability_probability: as_f64(opportunity.profitability_probability),
            entry_basis_bps: as_f64(opportunity.entry_basis_bps),
            leverage: as_f64(opportunity.leverage),
            required_capital: as_f64(opportunity.required_capital),
            available_capacity: as_f64(opportunity.available_capacity),
            reserved_notional: as_f64(assessment.reserved_notional),
            reserved_capital: as_f64(assessment.reserved_capital),
            capital_fraction: as_f64(assessment.capital_fraction),
            within_limits: assessment.within_limits,
            long_leg: LegDto::from_domain(&opportunity.long_leg),
            short_leg: LegDto::from_domain(&opportunity.short_leg),
            next_funding_at: opportunity.next_funding_at.to_rfc3339(),
        }
    }
}

/// The active risk limits (the §6.2 safe-starter template), as served by the API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RiskLimitsDto {
    /// Reference net equity used to size reservations.
    pub reference_equity: f64,
    /// Maximum deployed portfolio capital, as a fraction of equity.
    pub max_portfolio_fraction: f64,
    /// Maximum capital in one opportunity, as a fraction of equity.
    pub max_opportunity_fraction: f64,
    /// Maximum capital in one underlying, as a fraction of equity.
    pub max_underlying_fraction: f64,
    /// Maximum capital in one venue, as a fraction of equity.
    pub max_venue_fraction: f64,
    /// Maximum leverage on any perpetual leg.
    pub max_leverage: f64,
    /// Minimum route profitability probability.
    pub min_profitability: f64,
    /// Minimum expected net APR.
    pub min_net_apr: f64,
    /// Maximum entry basis magnitude, in basis points.
    pub max_entry_basis_bps: f64,
    /// Minimum depth inside the slippage cap, as a multiple of leg notional.
    pub min_depth_multiple: f64,
}

impl RiskLimitsDto {
    /// Project the domain risk limits onto the wire shape.
    #[must_use]
    pub fn from_domain(limits: &RiskLimits) -> Self {
        Self {
            reference_equity: as_f64(limits.reference_equity),
            max_portfolio_fraction: as_f64(limits.max_portfolio_fraction),
            max_opportunity_fraction: as_f64(limits.max_opportunity_fraction),
            max_underlying_fraction: as_f64(limits.max_underlying_fraction),
            max_venue_fraction: as_f64(limits.max_venue_fraction),
            max_leverage: as_f64(limits.max_leverage),
            min_profitability: as_f64(limits.min_profitability),
            min_net_apr: as_f64(limits.min_net_apr),
            max_entry_basis_bps: as_f64(limits.max_entry_basis_bps),
            min_depth_multiple: as_f64(limits.min_depth_multiple),
        }
    }
}
