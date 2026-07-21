use std::str::FromStr;

use chrono::{DateTime, Utc};
use funding_arb_domain::TradingMode;
use funding_arb_domain::market::{RouteType, Venue};
use funding_arb_domain::strategy::{
    AllocationMethod, AutoCompoundPolicy, Strategy, StrategyVersion,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Wire capital allocation configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AllocationDto {
    /// `FIXED`, `PERCENT_EQUITY`, or `BOUNDED_DYNAMIC`.
    pub method: String,
    /// Fixed notional or equity fraction.
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub value: Option<Decimal>,
    /// Dynamic lower bound.
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub minimum: Option<Decimal>,
    /// Dynamic upper bound.
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub maximum: Option<Decimal>,
}

/// Wire compounding policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutoCompoundDto {
    /// Whether paper profits are reinvested.
    pub enabled: bool,
    /// Positive PnL fraction to reinvest.
    #[serde(with = "rust_decimal::serde::str")]
    pub reinvest_fraction: Decimal,
    /// Profit threshold before reinvestment.
    #[serde(with = "rust_decimal::serde::str")]
    pub minimum_reinvest_amount: Decimal,
}

/// Immutable version payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyVersionDto {
    pub version: u32,
    pub creator: String,
    pub created_at: String,
    pub mode: String,
    pub route_types: Vec<String>,
    pub allowed_venues: Vec<String>,
    pub allowed_assets: Vec<String>,
    #[serde(with = "rust_decimal::serde::str")]
    pub min_net_apr: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub min_profitability: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub min_forecast_coverage: Decimal,
    pub max_time_to_funding_minutes: u32,
    #[serde(with = "rust_decimal::serde::str")]
    pub max_basis_bps: Decimal,
    pub allocation: AllocationDto,
    pub max_concurrent: u32,
    pub cooldown_seconds: u64,
    pub hold_seconds: u64,
    pub auto_compound: AutoCompoundDto,
    #[serde(with = "rust_decimal::serde::str")]
    pub exit_net_apr: Decimal,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub take_profit_fraction: Option<Decimal>,
    #[serde(default, with = "rust_decimal::serde::str_option")]
    pub stop_loss_fraction: Option<Decimal>,
    #[serde(with = "rust_decimal::serde::str")]
    pub max_drawdown_fraction: Decimal,
}

/// Strategy resource and create/update body.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyDto {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    pub current: StrategyVersionDto,
}

impl StrategyDto {
    /// Parse the wire configuration, rejecting unsupported live settings.
    pub fn to_domain(&self) -> Option<Strategy> {
        let current = &self.current;
        let allocation = match current.allocation.method.as_str() {
            "FIXED" => AllocationMethod::Fixed(current.allocation.value?),
            "PERCENT_EQUITY" => AllocationMethod::PercentEquity(current.allocation.value?),
            "BOUNDED_DYNAMIC" => AllocationMethod::BoundedDynamic {
                minimum: current.allocation.minimum?,
                maximum: current.allocation.maximum?,
            },
            _ => return None,
        };
        Some(Strategy {
            id: self.id.clone(),
            name: self.name.clone(),
            enabled: self.enabled,
            current: StrategyVersion {
                version: current.version,
                creator: current.creator.clone(),
                created_at: DateTime::parse_from_rfc3339(&current.created_at)
                    .ok()?
                    .with_timezone(&Utc),
                mode: TradingMode::from_str(&current.mode).ok()?,
                route_types: current
                    .route_types
                    .iter()
                    .map(|value| RouteType::from_str(value).ok())
                    .collect::<Option<Vec<_>>>()?,
                allowed_venues: current
                    .allowed_venues
                    .iter()
                    .map(|value| Venue::from_str(value).ok())
                    .collect::<Option<Vec<_>>>()?,
                allowed_assets: current.allowed_assets.clone(),
                min_net_apr: current.min_net_apr,
                min_profitability: current.min_profitability,
                min_forecast_coverage: current.min_forecast_coverage,
                max_time_to_funding_minutes: current.max_time_to_funding_minutes,
                max_basis_bps: current.max_basis_bps,
                allocation,
                max_concurrent: current.max_concurrent,
                cooldown_seconds: current.cooldown_seconds,
                hold_seconds: current.hold_seconds,
                auto_compound: AutoCompoundPolicy {
                    enabled: current.auto_compound.enabled,
                    reinvest_fraction: current.auto_compound.reinvest_fraction,
                    minimum_reinvest_amount: current.auto_compound.minimum_reinvest_amount,
                },
                exit_net_apr: current.exit_net_apr,
                take_profit_fraction: current.take_profit_fraction,
                stop_loss_fraction: current.stop_loss_fraction,
                max_drawdown_fraction: current.max_drawdown_fraction,
            },
        })
    }

    #[must_use]
    pub fn from_domain(strategy: &Strategy) -> Self {
        let allocation = match strategy.current.allocation {
            AllocationMethod::Fixed(value) => AllocationDto {
                method: "FIXED".into(),
                value: Some(value),
                minimum: None,
                maximum: None,
            },
            AllocationMethod::PercentEquity(value) => AllocationDto {
                method: "PERCENT_EQUITY".into(),
                value: Some(value),
                minimum: None,
                maximum: None,
            },
            AllocationMethod::BoundedDynamic { minimum, maximum } => AllocationDto {
                method: "BOUNDED_DYNAMIC".into(),
                value: None,
                minimum: Some(minimum),
                maximum: Some(maximum),
            },
        };
        Self {
            id: strategy.id.clone(),
            name: strategy.name.clone(),
            enabled: strategy.enabled,
            current: StrategyVersionDto {
                version: strategy.current.version,
                creator: strategy.current.creator.clone(),
                created_at: strategy.current.created_at.to_rfc3339(),
                mode: if strategy.current.mode.is_paper() {
                    "PAPER".into()
                } else {
                    "LIVE".into()
                },
                route_types: strategy
                    .current
                    .route_types
                    .iter()
                    .map(|value| value.as_str().into())
                    .collect(),
                allowed_venues: strategy
                    .current
                    .allowed_venues
                    .iter()
                    .map(|value| value.as_str().into())
                    .collect(),
                allowed_assets: strategy.current.allowed_assets.clone(),
                min_net_apr: strategy.current.min_net_apr,
                min_profitability: strategy.current.min_profitability,
                min_forecast_coverage: strategy.current.min_forecast_coverage,
                max_time_to_funding_minutes: strategy.current.max_time_to_funding_minutes,
                max_basis_bps: strategy.current.max_basis_bps,
                allocation,
                max_concurrent: strategy.current.max_concurrent,
                cooldown_seconds: strategy.current.cooldown_seconds,
                hold_seconds: strategy.current.hold_seconds,
                auto_compound: AutoCompoundDto {
                    enabled: strategy.current.auto_compound.enabled,
                    reinvest_fraction: strategy.current.auto_compound.reinvest_fraction,
                    minimum_reinvest_amount: strategy.current.auto_compound.minimum_reinvest_amount,
                },
                exit_net_apr: strategy.current.exit_net_apr,
                take_profit_fraction: strategy.current.take_profit_fraction,
                stop_loss_fraction: strategy.current.stop_loss_fraction,
                max_drawdown_fraction: strategy.current.max_drawdown_fraction,
            },
        }
    }
}
