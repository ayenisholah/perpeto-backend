use chrono::{DateTime, Utc};
use funding_arb_domain::opportunity::Opportunity;
use funding_arb_domain::position::PositionState;
use funding_arb_domain::risk::RiskLimits;
use funding_arb_domain::strategy::Strategy;
use rust_decimal::Decimal;

/// An admitted, sized automatic paper entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenIntent {
    pub strategy_id: String,
    pub route_id: String,
    pub notional: Decimal,
}

/// Stateless deterministic strategy evaluator.
pub struct StrategyService;

impl StrategyService {
    #[must_use]
    pub fn evaluate(
        strategy: &Strategy,
        opportunities: &[Opportunity],
        positions: &[PositionSummary],
        limits: &RiskLimits,
        free_equity: Decimal,
        now: DateTime<Utc>,
    ) -> Vec<OpenIntent> {
        let active = positions
            .iter()
            .filter(|position| {
                position.strategy_id.as_deref() == Some(strategy.id.as_str())
                    && position.state != PositionState::Closed
            })
            .count();
        if active >= strategy.current.max_concurrent as usize {
            return Vec::new();
        }
        let last = positions
            .iter()
            .filter(|position| position.strategy_id.as_deref() == Some(strategy.id.as_str()))
            .map(|position| position.at)
            .max();
        if last.is_some_and(|at| {
            now.signed_duration_since(at).num_seconds() < strategy.current.cooldown_seconds as i64
        }) {
            return Vec::new();
        }
        let profit: Decimal = positions
            .iter()
            .filter(|position| position.strategy_id.as_deref() == Some(strategy.id.as_str()))
            .map(|position| position.realized_pnl.max(Decimal::ZERO))
            .sum();
        opportunities
            .iter()
            .filter(|opportunity| strategy.admits(opportunity, limits, now))
            .take(strategy.current.max_concurrent as usize - active)
            .map(|opportunity| OpenIntent {
                strategy_id: strategy.id.clone(),
                route_id: opportunity.route_id.clone(),
                notional: strategy.entry_notional(opportunity, free_equity, profit),
            })
            .collect()
    }
}

/// Minimal position projection used by the evaluator.
#[derive(Clone, Debug)]
pub struct PositionSummary {
    pub strategy_id: Option<String>,
    pub state: PositionState,
    pub at: DateTime<Utc>,
    pub realized_pnl: Decimal,
}
