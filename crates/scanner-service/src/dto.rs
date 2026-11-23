//! Wire DTOs for the opportunity scan (decimals rendered as JSON strings).
//!
//! `OpportunityDto` is used for both the list and the detail view, and is also
//! the serialized payload persisted per opportunity, so it derives `Deserialize`.

use funding_arb_domain::opportunity::Opportunity;
use funding_arb_domain::risk::{RiskAssessment, RiskLimits};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

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
    #[serde(with = "rust_decimal::serde::str")]
    pub predicted_rate: Decimal,
    /// Lower (p10) annualized-rate bound.
    #[serde(with = "rust_decimal::serde::str")]
    pub p10_rate: Decimal,
    /// Upper (p90) annualized-rate bound.
    #[serde(with = "rust_decimal::serde::str")]
    pub p90_rate: Decimal,
    /// Probability the rate keeps its sign.
    #[serde(with = "rust_decimal::serde::str")]
    pub direction_probability: Decimal,
    /// RFC 3339 timestamp of the leg's next funding settlement.
    pub next_funding_at: String,
    /// RFC 3339 timestamp of the market observation the leg was ranked from.
    /// Absent on scans stored before client 0.23.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
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
    #[serde(with = "rust_decimal::serde::str")]
    pub gross_funding_apr: Decimal,
    /// Annualized entry cost.
    #[serde(with = "rust_decimal::serde::str")]
    pub entry_cost: Decimal,
    /// Annualized exit cost.
    #[serde(with = "rust_decimal::serde::str")]
    pub exit_cost: Decimal,
    /// Annualized carry cost.
    #[serde(with = "rust_decimal::serde::str")]
    pub carry_cost: Decimal,
    /// Risk buffer deducted from APR.
    #[serde(with = "rust_decimal::serde::str")]
    pub risk_buffer: Decimal,
    /// Gross funding APR required to break even.
    #[serde(with = "rust_decimal::serde::str")]
    pub break_even_funding: Decimal,
    /// Net APR after costs.
    #[serde(with = "rust_decimal::serde::str")]
    pub net_apr: Decimal,
    /// Expected net PnL (USD) over the holding period.
    #[serde(with = "rust_decimal::serde::str")]
    pub expected_net_pnl: Decimal,
    /// Probability net PnL is positive, in `[0, 1]`.
    #[serde(with = "rust_decimal::serde::str")]
    pub profitability_probability: Decimal,
    /// Entry basis between legs, in basis points.
    #[serde(with = "rust_decimal::serde::str")]
    pub entry_basis_bps: Decimal,
    /// Assumed leverage on each perpetual leg.
    #[serde(with = "rust_decimal::serde::str")]
    pub leverage: Decimal,
    /// Capital (USD) tied up at full capacity.
    #[serde(with = "rust_decimal::serde::str")]
    pub required_capital: Decimal,
    /// Depth-limited executable capacity (USD).
    #[serde(with = "rust_decimal::serde::str")]
    pub available_capacity: Decimal,
    /// Notional (USD) reserved under the per-opportunity cap.
    #[serde(with = "rust_decimal::serde::str")]
    pub reserved_notional: Decimal,
    /// Capital (USD) reserved.
    #[serde(with = "rust_decimal::serde::str")]
    pub reserved_capital: Decimal,
    /// Reserved capital as a fraction of reference equity.
    #[serde(with = "rust_decimal::serde::str")]
    pub capital_fraction: Decimal,
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
            predicted_rate: leg.forecast.predicted_settlement_rate,
            p10_rate: leg.forecast.p10_rate,
            p90_rate: leg.forecast.p90_rate,
            direction_probability: leg.forecast.direction_probability,
            next_funding_at: leg.forecast.next_funding_at.to_rfc3339(),
            observed_at: Some(leg.forecast.input_freshness.to_rfc3339()),
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
            gross_funding_apr: opportunity.gross_funding_apr,
            entry_cost: opportunity.entry_cost,
            exit_cost: opportunity.exit_cost,
            carry_cost: opportunity.carry_cost,
            risk_buffer: opportunity.risk_buffer,
            break_even_funding: opportunity.break_even_funding,
            net_apr: opportunity.net_apr,
            expected_net_pnl: opportunity.expected_net_pnl,
            profitability_probability: opportunity.profitability_probability,
            entry_basis_bps: opportunity.entry_basis_bps,
            leverage: opportunity.leverage,
            required_capital: opportunity.required_capital,
            available_capacity: opportunity.available_capacity,
            reserved_notional: assessment.reserved_notional,
            reserved_capital: assessment.reserved_capital,
            capital_fraction: assessment.capital_fraction,
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
    #[serde(with = "rust_decimal::serde::str")]
    pub reference_equity: Decimal,
    /// Maximum deployed portfolio capital, as a fraction of equity.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_portfolio_fraction: Decimal,
    /// Maximum capital in one opportunity, as a fraction of equity.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_opportunity_fraction: Decimal,
    /// Maximum capital in one underlying, as a fraction of equity.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_underlying_fraction: Decimal,
    /// Maximum capital in one venue, as a fraction of equity.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_venue_fraction: Decimal,
    /// Maximum leverage on any perpetual leg.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_leverage: Decimal,
    /// Minimum route profitability probability.
    #[serde(with = "rust_decimal::serde::str")]
    pub min_profitability: Decimal,
    /// Minimum expected net APR.
    #[serde(with = "rust_decimal::serde::str")]
    pub min_net_apr: Decimal,
    /// Maximum entry basis magnitude, in basis points.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_entry_basis_bps: Decimal,
    /// Minimum depth inside the slippage cap, as a multiple of leg notional.
    #[serde(with = "rust_decimal::serde::str")]
    pub min_depth_multiple: Decimal,
    /// Effective cap on unmatched (residual) two-leg exposure, in USD.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_unmatched_usd: Decimal,
    /// Maximum realized daily loss as a fraction of reference equity.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_daily_loss_fraction: Decimal,
    /// Maximum peak-to-current equity drawdown fraction.
    #[serde(with = "rust_decimal::serde::str")]
    pub max_drawdown_fraction: Decimal,
}

impl RiskLimitsDto {
    /// Project the domain risk limits onto the wire shape.
    #[must_use]
    pub fn from_domain(limits: &RiskLimits) -> Self {
        Self {
            reference_equity: limits.reference_equity,
            max_portfolio_fraction: limits.max_portfolio_fraction,
            max_opportunity_fraction: limits.max_opportunity_fraction,
            max_underlying_fraction: limits.max_underlying_fraction,
            max_venue_fraction: limits.max_venue_fraction,
            max_leverage: limits.max_leverage,
            min_profitability: limits.min_profitability,
            min_net_apr: limits.min_net_apr,
            max_entry_basis_bps: limits.max_entry_basis_bps,
            min_depth_multiple: limits.min_depth_multiple,
            max_unmatched_usd: limits.max_unmatched(),
            max_daily_loss_fraction: limits.max_daily_loss_fraction,
            max_drawdown_fraction: limits.max_drawdown_fraction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LegDto;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn decimal_wire_values_round_trip_without_precision_loss() {
        let exact = Decimal::from_str("0.123456789012345678").expect("exact decimal");
        let leg = LegDto {
            venue: "BYBIT".to_owned(),
            predicted_rate: exact,
            p10_rate: exact,
            p90_rate: exact,
            direction_probability: exact,
            next_funding_at: "2026-07-19T08:00:00Z".to_owned(),
            observed_at: None,
        };

        let encoded = serde_json::to_string(&leg).expect("serialize leg");
        assert!(encoded.contains("\"predicted_rate\":\"0.123456789012345678\""));
        let decoded: LegDto = serde_json::from_str(&encoded).expect("deserialize leg");
        assert_eq!(decoded.predicted_rate, exact);
    }

    #[test]
    fn legs_stored_before_observed_at_still_decode() {
        let legacy = r#"{"venue":"BYBIT","predicted_rate":"0.1","p10_rate":"0.05","p90_rate":"0.15","direction_probability":"0.9","next_funding_at":"2026-07-19T08:00:00Z"}"#;
        let decoded: LegDto = serde_json::from_str(legacy).expect("legacy leg");
        assert_eq!(decoded.observed_at, None);
        let encoded = serde_json::to_string(&decoded).expect("encode");
        assert!(!encoded.contains("observed_at"));
    }
}
