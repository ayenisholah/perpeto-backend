//! Wire DTOs for the opportunity scan (numbers rendered as plain JSON floats).

use funding_arb_domain::opportunity::Opportunity;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;

use crate::service::ScanResult;

fn as_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

/// A ranked scan with provenance, as served by the API.
#[derive(Clone, Debug, Serialize)]
pub struct OpportunityScanDto {
    /// RFC 3339 timestamp of when the scan was produced.
    pub scanned_at: String,
    /// The forecasting model version.
    pub model_version: String,
    /// Ranked, eligible opportunities.
    pub opportunities: Vec<OpportunityDto>,
}

/// A single costed, ranked opportunity.
#[derive(Clone, Debug, Serialize)]
pub struct OpportunityDto {
    /// Stable route identifier.
    pub route_id: String,
    /// `SPOT_PERP` or `PERP_PERP`.
    pub route_type: String,
    /// The underlying asset.
    pub underlying: String,
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
    /// Net APR after costs.
    pub net_apr: f64,
    /// Expected net PnL (USD) over the holding period.
    pub expected_net_pnl: f64,
    /// Probability net PnL is positive, in `[0, 1]`.
    pub profitability_probability: f64,
    /// Depth-limited executable capacity (USD).
    pub available_capacity: f64,
    /// RFC 3339 timestamp of the next funding settlement.
    pub next_funding_at: String,
}

impl OpportunityDto {
    /// Project a domain opportunity onto the wire shape.
    #[must_use]
    pub fn from_domain(opportunity: &Opportunity) -> Self {
        Self {
            route_id: opportunity.route_id.clone(),
            route_type: opportunity.route_type.as_str().to_owned(),
            underlying: opportunity.underlying.clone(),
            long_venue: opportunity.long_leg.forecast.venue.as_str().to_owned(),
            short_venue: opportunity.short_leg.forecast.venue.as_str().to_owned(),
            gross_funding_apr: as_f64(opportunity.gross_funding_apr),
            entry_cost: as_f64(opportunity.entry_cost),
            exit_cost: as_f64(opportunity.exit_cost),
            carry_cost: as_f64(opportunity.carry_cost),
            risk_buffer: as_f64(opportunity.risk_buffer),
            net_apr: as_f64(opportunity.net_apr),
            expected_net_pnl: as_f64(opportunity.expected_net_pnl),
            profitability_probability: as_f64(opportunity.profitability_probability),
            available_capacity: as_f64(opportunity.available_capacity),
            next_funding_at: opportunity.next_funding_at.to_rfc3339(),
        }
    }
}

impl OpportunityScanDto {
    /// Project a scan result onto the wire shape.
    #[must_use]
    pub fn from_result(result: &ScanResult) -> Self {
        Self {
            scanned_at: result.scanned_at.to_rfc3339(),
            model_version: result.model_version.to_owned(),
            opportunities: result.opportunities.iter().map(OpportunityDto::from_domain).collect(),
        }
    }
}
