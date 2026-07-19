//! Risk limits and per-opportunity reservations (PRODUCT_SPEC §6).
//!
//! [`RiskLimits::default`] is the §6.2 safe-starter deployment template. The
//! scanner uses [`RiskLimits::admits`] as its hard eligibility gate and
//! [`RiskLimits::assess`] to compute the paper capital **reservation** each
//! opportunity would take under the per-opportunity cap. Live position/order
//! reservations arrive with the execution slice.

use rust_decimal::Decimal;

use crate::opportunity::Opportunity;

/// Organization risk limits. Defaults are the §6.2 safe-starter template.
#[derive(Clone, Copy, Debug)]
pub struct RiskLimits {
    /// Reference net equity (deployment base currency) used to size reservations.
    pub reference_equity: Decimal,
    /// Maximum deployed portfolio capital, as a fraction of equity.
    pub max_portfolio_fraction: Decimal,
    /// Maximum capital in one opportunity, as a fraction of equity.
    pub max_opportunity_fraction: Decimal,
    /// Maximum capital in one underlying, as a fraction of equity.
    pub max_underlying_fraction: Decimal,
    /// Maximum capital in one venue, as a fraction of equity.
    pub max_venue_fraction: Decimal,
    /// Maximum leverage on any perpetual leg.
    pub max_leverage: Decimal,
    /// Minimum route profitability probability.
    pub min_profitability: Decimal,
    /// Minimum expected net APR.
    pub min_net_apr: Decimal,
    /// Maximum entry basis magnitude, in basis points.
    pub max_entry_basis_bps: Decimal,
    /// Minimum depth inside the slippage cap, as a multiple of leg notional.
    pub min_depth_multiple: Decimal,
    /// Minimum executable notional for a route to be eligible.
    pub min_reserved_notional: Decimal,
    /// Absolute cap on unmatched (residual) two-leg exposure, in USD.
    pub max_unmatched_usd: Decimal,
    /// Cap on unmatched exposure as a fraction of reference equity.
    pub max_unmatched_fraction: Decimal,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            reference_equity: Decimal::from(1_000_000),
            max_portfolio_fraction: Decimal::new(70, 2),  // 0.70
            max_opportunity_fraction: Decimal::new(10, 2), // 0.10
            max_underlying_fraction: Decimal::new(20, 2),  // 0.20
            max_venue_fraction: Decimal::new(35, 2),       // 0.35
            max_leverage: Decimal::from(2),                // 2.0x
            min_profitability: Decimal::new(70, 2),        // 0.70
            min_net_apr: Decimal::new(15, 2),              // 0.15
            max_entry_basis_bps: Decimal::from(100),       // 100 bps
            min_depth_multiple: Decimal::from(5),          // 5x
            min_reserved_notional: Decimal::from(1_000),
            max_unmatched_usd: Decimal::from(1_000),       // USD 1,000
            max_unmatched_fraction: Decimal::new(5, 3),    // 0.5% of equity
        }
    }
}

/// The paper capital reservation and limit status for one opportunity.
#[derive(Clone, Copy, Debug)]
pub struct RiskAssessment {
    /// Whether the opportunity clears every hard risk gate.
    pub within_limits: bool,
    /// Notional (USD) reserved under the per-opportunity cap.
    pub reserved_notional: Decimal,
    /// Capital (USD) tied up by the reserved notional.
    pub reserved_capital: Decimal,
    /// Reserved capital as a fraction of reference equity.
    pub capital_fraction: Decimal,
}

impl RiskLimits {
    /// Whether an opportunity clears every hard eligibility gate.
    #[must_use]
    pub fn admits(&self, opportunity: &Opportunity) -> bool {
        opportunity.net_apr >= self.min_net_apr
            && opportunity.profitability_probability >= self.min_profitability
            && opportunity.entry_basis_bps <= self.max_entry_basis_bps
            && opportunity.leverage <= self.max_leverage
            && opportunity.available_capacity >= self.min_reserved_notional
    }

    /// The effective unmatched-exposure cap: the lesser of the absolute USD cap
    /// and the equity-fraction cap (PRODUCT_SPEC §6.2).
    #[must_use]
    pub fn max_unmatched(&self) -> Decimal {
        (self.reference_equity * self.max_unmatched_fraction).min(self.max_unmatched_usd)
    }

    /// Compute the paper reservation and limit status for an opportunity.
    #[must_use]
    pub fn assess(&self, opportunity: &Opportunity) -> RiskAssessment {
        let per_opportunity_cap = self.reference_equity * self.max_opportunity_fraction;
        let reserved_notional = opportunity.available_capacity.min(per_opportunity_cap);
        let reserved_capital = if opportunity.available_capacity.is_zero() {
            Decimal::ZERO
        } else {
            opportunity.required_capital * (reserved_notional / opportunity.available_capacity)
        };
        let capital_fraction = if self.reference_equity.is_zero() {
            Decimal::ZERO
        } else {
            reserved_capital / self.reference_equity
        };
        RiskAssessment {
            within_limits: self.admits(opportunity),
            reserved_notional,
            reserved_capital,
            capital_fraction,
        }
    }
}
