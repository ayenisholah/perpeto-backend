use std::str::FromStr;

use chrono::{DateTime, Utc};
use funding_arb_domain::TradingMode;
use funding_arb_domain::market::{RouteType, Venue};
use funding_arb_domain::strategy::{
    AllocationMethod, AutoCompoundPolicy, Strategy, StrategyVersion,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

fn decimal(value: f64) -> Decimal {
    Decimal::from_f64_retain(value).unwrap_or_default()
}

fn number(value: Decimal) -> f64 {
    value.to_f64().unwrap_or_default()
}

/// Wire capital allocation configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AllocationDto {
    /// `FIXED`, `PERCENT_EQUITY`, or `BOUNDED_DYNAMIC`.
    pub method: String,
    /// Fixed notional or equity fraction.
    pub value: Option<f64>,
    /// Dynamic lower bound.
    pub minimum: Option<f64>,
    /// Dynamic upper bound.
    pub maximum: Option<f64>,
}

/// Wire compounding policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutoCompoundDto {
    /// Whether paper profits are reinvested.
    pub enabled: bool,
    /// Positive PnL fraction to reinvest.
    pub reinvest_fraction: f64,
    /// Profit threshold before reinvestment.
    pub minimum_reinvest_amount: f64,
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
    pub min_net_apr: f64,
    pub min_profitability: f64,
    pub min_forecast_coverage: f64,
    pub max_time_to_funding_minutes: u32,
    pub max_basis_bps: f64,
    pub allocation: AllocationDto,
    pub max_concurrent: u32,
    pub cooldown_seconds: u64,
    pub hold_seconds: u64,
    pub auto_compound: AutoCompoundDto,
    pub exit_net_apr: f64,
    pub take_profit_fraction: Option<f64>,
    pub stop_loss_fraction: Option<f64>,
    pub max_drawdown_fraction: f64,
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
            "FIXED" => AllocationMethod::Fixed(decimal(current.allocation.value?)),
            "PERCENT_EQUITY" => AllocationMethod::PercentEquity(decimal(current.allocation.value?)),
            "BOUNDED_DYNAMIC" => AllocationMethod::BoundedDynamic {
                minimum: decimal(current.allocation.minimum?),
                maximum: decimal(current.allocation.maximum?),
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
                min_net_apr: decimal(current.min_net_apr),
                min_profitability: decimal(current.min_profitability),
                min_forecast_coverage: decimal(current.min_forecast_coverage),
                max_time_to_funding_minutes: current.max_time_to_funding_minutes,
                max_basis_bps: decimal(current.max_basis_bps),
                allocation,
                max_concurrent: current.max_concurrent,
                cooldown_seconds: current.cooldown_seconds,
                hold_seconds: current.hold_seconds,
                auto_compound: AutoCompoundPolicy {
                    enabled: current.auto_compound.enabled,
                    reinvest_fraction: decimal(current.auto_compound.reinvest_fraction),
                    minimum_reinvest_amount: decimal(current.auto_compound.minimum_reinvest_amount),
                },
                exit_net_apr: decimal(current.exit_net_apr),
                take_profit_fraction: current.take_profit_fraction.map(decimal),
                stop_loss_fraction: current.stop_loss_fraction.map(decimal),
                max_drawdown_fraction: decimal(current.max_drawdown_fraction),
            },
        })
    }

    #[must_use]
    pub fn from_domain(strategy: &Strategy) -> Self {
        let allocation = match strategy.current.allocation {
            AllocationMethod::Fixed(value) => AllocationDto {
                method: "FIXED".into(),
                value: Some(number(value)),
                minimum: None,
                maximum: None,
            },
            AllocationMethod::PercentEquity(value) => AllocationDto {
                method: "PERCENT_EQUITY".into(),
                value: Some(number(value)),
                minimum: None,
                maximum: None,
            },
            AllocationMethod::BoundedDynamic { minimum, maximum } => AllocationDto {
                method: "BOUNDED_DYNAMIC".into(),
                value: None,
                minimum: Some(number(minimum)),
                maximum: Some(number(maximum)),
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
                min_net_apr: number(strategy.current.min_net_apr),
                min_profitability: number(strategy.current.min_profitability),
                min_forecast_coverage: number(strategy.current.min_forecast_coverage),
                max_time_to_funding_minutes: strategy.current.max_time_to_funding_minutes,
                max_basis_bps: number(strategy.current.max_basis_bps),
                allocation,
                max_concurrent: strategy.current.max_concurrent,
                cooldown_seconds: strategy.current.cooldown_seconds,
                hold_seconds: strategy.current.hold_seconds,
                auto_compound: AutoCompoundDto {
                    enabled: strategy.current.auto_compound.enabled,
                    reinvest_fraction: number(strategy.current.auto_compound.reinvest_fraction),
                    minimum_reinvest_amount: number(
                        strategy.current.auto_compound.minimum_reinvest_amount,
                    ),
                },
                exit_net_apr: number(strategy.current.exit_net_apr),
                take_profit_fraction: strategy.current.take_profit_fraction.map(number),
                stop_loss_fraction: strategy.current.stop_loss_fraction.map(number),
                max_drawdown_fraction: number(strategy.current.max_drawdown_fraction),
            },
        }
    }
}
