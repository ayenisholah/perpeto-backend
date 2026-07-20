//! Versioned paper-strategy configuration and admission rules.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::TradingMode;
use crate::market::{RouteType, Venue};
use crate::opportunity::Opportunity;
use crate::risk::RiskLimits;

/// Capital sizing selected by a strategy version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AllocationMethod {
    /// Fixed USD notional.
    Fixed(Decimal),
    /// Fraction of free equity.
    PercentEquity(Decimal),
    /// Dynamic sizing constrained to an inclusive USD range.
    BoundedDynamic { minimum: Decimal, maximum: Decimal },
}

/// Rules for reinvesting paper profits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoCompoundPolicy {
    /// Whether realized profit can increase future allocation.
    pub enabled: bool,
    /// Fraction of positive realized PnL to reinvest.
    pub reinvest_fraction: Decimal,
    /// Minimum accumulated profit before reinvestment.
    pub minimum_reinvest_amount: Decimal,
}

/// Immutable executable configuration snapshot.
#[derive(Clone, Debug)]
pub struct StrategyVersion {
    /// Monotonic version number.
    pub version: u32,
    /// Creator user id.
    pub creator: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Paper/live mode. M2 accepts paper only.
    pub mode: TradingMode,
    /// Allowed route kinds; empty means all.
    pub route_types: Vec<RouteType>,
    /// Allowed venues; empty means all.
    pub allowed_venues: Vec<Venue>,
    /// Allowed underlying symbols; empty means all.
    pub allowed_assets: Vec<String>,
    /// Minimum expected net APR.
    pub min_net_apr: Decimal,
    /// Minimum profitability probability.
    pub min_profitability: Decimal,
    /// Minimum average forecast direction probability.
    pub min_forecast_coverage: Decimal,
    /// Maximum wait until next funding, in minutes.
    pub max_time_to_funding_minutes: u32,
    /// Maximum absolute entry basis in basis points.
    pub max_basis_bps: Decimal,
    /// Capital sizing method.
    pub allocation: AllocationMethod,
    /// Maximum simultaneously active positions.
    pub max_concurrent: u32,
    /// Minimum delay after an entry/exit/failure.
    pub cooldown_seconds: u64,
    /// Target hold time.
    pub hold_seconds: u64,
    /// Reinvestment rules.
    pub auto_compound: AutoCompoundPolicy,
    /// Strategy exit APR; must not weaken the organization threshold.
    pub exit_net_apr: Decimal,
    /// Optional take-profit fraction.
    pub take_profit_fraction: Option<Decimal>,
    /// Optional stop-loss fraction.
    pub stop_loss_fraction: Option<Decimal>,
    /// Maximum strategy drawdown fraction.
    pub max_drawdown_fraction: Decimal,
}

/// A named versioned strategy.
#[derive(Clone, Debug)]
pub struct Strategy {
    /// Stable strategy id.
    pub id: String,
    /// Operator-facing name.
    pub name: String,
    /// Whether new entries may be generated.
    pub enabled: bool,
    /// Current immutable configuration.
    pub current: StrategyVersion,
}

impl Strategy {
    /// Admit only opportunities that pass organization limits and this stricter version.
    #[must_use]
    pub fn admits(
        &self,
        opportunity: &Opportunity,
        limits: &RiskLimits,
        now: DateTime<Utc>,
    ) -> bool {
        if !self.enabled || !self.current.mode.is_paper() || !limits.admits(opportunity) {
            return false;
        }
        let version = &self.current;
        let coverage = (opportunity.long_leg.forecast.direction_probability
            + opportunity.short_leg.forecast.direction_probability)
            / Decimal::from(2);
        let minutes = opportunity
            .next_funding_at
            .signed_duration_since(now)
            .num_minutes()
            .max(0);
        (version.route_types.is_empty() || version.route_types.contains(&opportunity.route_type))
            && (version.allowed_assets.is_empty()
                || version.allowed_assets.contains(&opportunity.underlying))
            && (version.allowed_venues.is_empty()
                || (version
                    .allowed_venues
                    .contains(&opportunity.long_leg.forecast.venue)
                    && version
                        .allowed_venues
                        .contains(&opportunity.short_leg.forecast.venue)))
            && version.min_net_apr >= limits.min_net_apr
            && opportunity.net_apr >= version.min_net_apr
            && version.min_profitability >= limits.min_profitability
            && opportunity.profitability_probability >= version.min_profitability
            && coverage >= version.min_forecast_coverage
            && minutes <= i64::from(version.max_time_to_funding_minutes)
            && version.max_basis_bps <= limits.max_entry_basis_bps
            && opportunity.entry_basis_bps <= version.max_basis_bps
            && version.max_drawdown_fraction <= limits.max_drawdown_fraction
    }

    /// Size one admitted entry without exceeding route or organization capacity.
    #[must_use]
    pub fn entry_notional(
        &self,
        opportunity: &Opportunity,
        free_equity: Decimal,
        compound_profit: Decimal,
    ) -> Decimal {
        let base = match &self.current.allocation {
            AllocationMethod::Fixed(value) => *value,
            AllocationMethod::PercentEquity(fraction) => free_equity * *fraction,
            AllocationMethod::BoundedDynamic { minimum, maximum } => {
                (free_equity * Decimal::new(1, 1)).clamp(*minimum, *maximum)
            }
        };
        let compound = if self.current.auto_compound.enabled
            && compound_profit >= self.current.auto_compound.minimum_reinvest_amount
        {
            compound_profit * self.current.auto_compound.reinvest_fraction
        } else {
            Decimal::ZERO
        };
        (base + compound).min(opportunity.available_capacity)
    }
}
