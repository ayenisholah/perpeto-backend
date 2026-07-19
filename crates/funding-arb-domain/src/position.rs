//! Paper position lifecycle: the coordinated open path (PRODUCT_SPEC §5).
//!
//! This module opens a **paper** arbitrage position from a ranked
//! [`Opportunity`]: it sizes both legs, simulates a deterministic paired taker
//! fill, measures residual (unmatched) delta, and gates it against the
//! configured risk cap. It is pure and network-free — the same lifecycle will
//! later drive live execution behind a connector. Funding capture, close, and
//! re-hedge arrive in the next slice; this slice only reaches [`Open`].
//!
//! [`Open`]: PositionState::Open

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::market::{RouteType, Venue};
use crate::opportunity::{CostModel, FundingDirection, Opportunity};
use crate::risk::RiskLimits;

fn abs(value: Decimal) -> Decimal {
    if value.is_sign_negative() {
        -value
    } else {
        value
    }
}

/// Round a quantity down to a whole multiple of `step` (a venue lot size).
/// A non-positive `step` leaves the quantity unrounded.
fn round_to_step(quantity: Decimal, step: Decimal) -> Decimal {
    if step <= Decimal::ZERO {
        return quantity;
    }
    (quantity / step).floor() * step
}

/// The position lifecycle states (PRODUCT_SPEC §5.2).
///
/// This slice only transitions along the open path
/// (`Qualified → Reserved → Opening → Open | OpenFailed`). The later states are
/// declared so the persisted contract is stable when the close/re-hedge slice
/// begins driving them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionState {
    /// A candidate route selected for opening.
    Qualified,
    /// Portfolio/risk capital reserved for the route.
    Reserved,
    /// Both legs are being filled.
    Opening,
    /// Both legs filled with residual delta within tolerance.
    Open,
    /// Opening aborted (e.g. residual delta exceeded the cap).
    OpenFailed,
    /// Reserved for the close/re-hedge slice.
    Rehedging,
    /// Reserved for the close/re-hedge slice.
    Closing,
    /// Reserved for the close/re-hedge slice.
    Closed,
}

impl PositionState {
    /// The canonical wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Qualified => "QUALIFIED",
            Self::Reserved => "RESERVED",
            Self::Opening => "OPENING",
            Self::Open => "OPEN",
            Self::OpenFailed => "OPEN_FAILED",
            Self::Rehedging => "REHEDGING",
            Self::Closing => "CLOSING",
            Self::Closed => "CLOSED",
        }
    }

    /// Whether a direct transition to `next` is permitted by the state machine.
    #[must_use]
    pub fn can_transition(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Qualified, Self::Reserved)
                | (Self::Reserved, Self::Opening)
                | (Self::Opening, Self::Open)
                | (Self::Opening, Self::OpenFailed)
                // Forward-compatible edges exercised by the next slice:
                | (Self::Open, Self::Rehedging)
                | (Self::Rehedging, Self::Open)
                | (Self::Open, Self::Closing)
                | (Self::Closing, Self::Closed)
        )
    }

    /// Advance to `next`, or report the rejected edge.
    ///
    /// # Errors
    /// Returns [`InvalidTransition`] when the edge is not permitted.
    pub fn try_transition(self, next: Self) -> Result<Self, InvalidTransition> {
        if self.can_transition(next) {
            Ok(next)
        } else {
            Err(InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

/// Raised when an illegal lifecycle transition is attempted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidTransition {
    /// The state transitioned from.
    pub from: PositionState,
    /// The rejected target state.
    pub to: PositionState,
}

/// The executable snapshot of one leg at open time (from the ranked scan).
#[derive(Clone, Debug)]
pub struct LegExecution {
    /// The venue-native symbol, e.g. `BTCUSDT`.
    pub symbol: String,
    /// The mark price used to size and fill the leg.
    pub mark_price: Decimal,
}

/// Deterministic paper-fill parameters.
#[derive(Clone, Copy, Debug)]
pub struct PaperFillConfig {
    /// The base-quantity lot step each leg is rounded down to.
    pub lot_step: Decimal,
}

impl Default for PaperFillConfig {
    fn default() -> Self {
        Self {
            lot_step: Decimal::new(1, 3),
        } // 0.001 base units
    }
}

/// A recorded order intent for one leg (persisted before the simulated fill).
#[derive(Clone, Debug)]
pub struct PaperOrder {
    /// Deterministic client order id (`fa-{short}-open-{leg}-{attempt}`).
    pub client_order_id: String,
    /// `BUY` for the long leg, `SELL` for the short leg.
    pub side: &'static str,
    /// The base quantity requested.
    pub target_qty: Decimal,
    /// The price cap the marketable IOC would not cross.
    pub price_cap: Decimal,
    /// Time in force — always `IOC` for the paired taker open.
    pub time_in_force: &'static str,
}

/// A simulated fill for one leg.
#[derive(Clone, Debug)]
pub struct PaperFill {
    /// The client order id the fill belongs to.
    pub client_order_id: String,
    /// The filled base quantity.
    pub qty: Decimal,
    /// The fill price after modeled slippage.
    pub price: Decimal,
    /// The taker fee paid on this fill (USD).
    pub fee: Decimal,
}

/// One leg of an opened paper position.
#[derive(Clone, Debug)]
pub struct PositionLeg {
    /// The leg's venue.
    pub venue: Venue,
    /// The venue-native symbol.
    pub symbol: String,
    /// Whether the leg is held long.
    pub is_long: bool,
    /// The base quantity targeted.
    pub target_qty: Decimal,
    /// The base quantity filled (equals target in paper).
    pub filled_qty: Decimal,
    /// The average fill price.
    pub avg_fill_price: Decimal,
    /// Total fees paid on the leg (USD).
    pub fees: Decimal,
    /// The order intents recorded for the leg.
    pub orders: Vec<PaperOrder>,
    /// The simulated fills for the leg.
    pub fills: Vec<PaperFill>,
}

/// A paper arbitrage position produced by [`open_paper`].
#[derive(Clone, Debug)]
pub struct ArbitragePosition {
    /// A UUID string identifying the position.
    pub id: String,
    /// The route the position was opened from.
    pub route_id: String,
    /// The underlying asset.
    pub underlying: String,
    /// Spot-perp or perp-perp.
    pub route_type: RouteType,
    /// The funding sign the route captures.
    pub funding_direction: FundingDirection,
    /// The terminal open-path state (`Open` or `OpenFailed`).
    pub state: PositionState,
    /// The long leg.
    pub long_leg: PositionLeg,
    /// The short leg.
    pub short_leg: PositionLeg,
    /// The requested notional (USD) sized per leg.
    pub target_notional: Decimal,
    /// Capital (USD) reserved for the position.
    pub reserved_capital: Decimal,
    /// Unmatched base-asset exposure valued in USD.
    pub residual_delta_usd: Decimal,
    /// When the position was opened.
    pub opened_at: DateTime<Utc>,
    /// The idempotency key the open was requested under.
    pub idempotency_key: String,
}

#[allow(clippy::too_many_arguments)]
fn leg(
    position_short: &str,
    label: &str,
    venue: Venue,
    is_long: bool,
    exec: &LegExecution,
    notional: Decimal,
    cost: &CostModel,
    fill: &PaperFillConfig,
) -> PositionLeg {
    let target_qty = if exec.mark_price.is_zero() {
        Decimal::ZERO
    } else {
        round_to_step(notional / exec.mark_price, fill.lot_step)
    };
    // A taker BUY crosses up by the modeled slippage; a taker SELL crosses down.
    let slip = if is_long {
        Decimal::ONE + cost.slippage
    } else {
        Decimal::ONE - cost.slippage
    };
    let price = exec.mark_price * slip;
    let fee = target_qty * price * cost.taker_fee;
    let client_order_id = format!("fa-{position_short}-open-{label}-1");
    let order = PaperOrder {
        client_order_id: client_order_id.clone(),
        side: if is_long { "BUY" } else { "SELL" },
        target_qty,
        price_cap: price,
        time_in_force: "IOC",
    };
    let fill_record = PaperFill {
        client_order_id,
        qty: target_qty,
        price,
        fee,
    };
    PositionLeg {
        venue,
        symbol: exec.symbol.clone(),
        is_long,
        target_qty,
        filled_qty: target_qty,
        avg_fill_price: price,
        fees: fee,
        orders: vec![order],
        fills: vec![fill_record],
    }
}

/// Open a paper position from a ranked opportunity and its executable legs.
///
/// Both legs are sized to `requested_notional` at their own mark, filled fully
/// as marketable IOC takers, and the residual base delta is valued in USD and
/// gated against [`RiskLimits::max_unmatched`]: within tolerance yields
/// [`PositionState::Open`], otherwise [`PositionState::OpenFailed`]. The result
/// is a pure function of its inputs, so an identical request reproduces an
/// identical position.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn open_paper(
    id: String,
    idempotency_key: String,
    opportunity: &Opportunity,
    long_exec: &LegExecution,
    short_exec: &LegExecution,
    requested_notional: Decimal,
    reserved_capital: Decimal,
    cost: &CostModel,
    fill: &PaperFillConfig,
    limits: &RiskLimits,
    now: DateTime<Utc>,
) -> ArbitragePosition {
    let position_short: String = id.chars().take(8).collect();
    let long_leg = leg(
        &position_short,
        "long",
        opportunity.long_leg.forecast.venue,
        true,
        long_exec,
        requested_notional,
        cost,
        fill,
    );
    let short_leg = leg(
        &position_short,
        "short",
        opportunity.short_leg.forecast.venue,
        false,
        short_exec,
        requested_notional,
        cost,
        fill,
    );

    let residual_base = long_leg.filled_qty - short_leg.filled_qty;
    let reference_price = (long_exec.mark_price + short_exec.mark_price) / Decimal::from(2);
    let residual_delta_usd = abs(residual_base) * reference_price;

    // Walk the open path; the residual gate decides Open vs OpenFailed.
    let state = if residual_delta_usd <= limits.max_unmatched() {
        PositionState::Open
    } else {
        PositionState::OpenFailed
    };

    ArbitragePosition {
        id,
        route_id: opportunity.route_id.clone(),
        underlying: opportunity.underlying.clone(),
        route_type: opportunity.route_type,
        funding_direction: opportunity.funding_direction,
        state,
        long_leg,
        short_leg,
        target_notional: requested_notional,
        reserved_capital,
        residual_delta_usd,
        opened_at: now,
        idempotency_key,
    }
}

#[cfg(test)]
mod tests {
    use super::{LegExecution, PaperFillConfig, PositionState, open_paper};
    use crate::forecast::{ForecastConfig, forecast};
    use crate::market::{FundingObservation, InstrumentRef, RouteType, Venue};
    use crate::opportunity::{CostModel, Opportunity, RouteLeg};
    use crate::risk::RiskLimits;
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    fn observation(
        venue: Venue,
        rate: Decimal,
        mark: Decimal,
        index: Decimal,
    ) -> FundingObservation {
        FundingObservation {
            instrument: InstrumentRef {
                venue,
                symbol: "BTCUSDT".into(),
                underlying: "BTC".into(),
            },
            funding_rate: rate,
            interval_hours: 8,
            mark_price: mark,
            index_price: index,
            available_notional: Decimal::from(100_000),
            observed_at: Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap(),
            next_funding_at: Utc.with_ymd_and_hms(2026, 7, 19, 8, 0, 0).unwrap(),
        }
    }

    fn opportunity() -> Opportunity {
        let config = ForecastConfig::default();
        let long = observation(
            Venue::Binance,
            Decimal::new(1, 4),
            Decimal::from(30_000),
            Decimal::from(30_000),
        );
        let short = observation(
            Venue::Bybit,
            Decimal::new(30, 4),
            Decimal::from(30_030),
            Decimal::from(30_000),
        );
        Opportunity::build(
            RouteType::PerpPerp,
            "BTC",
            RouteLeg {
                is_long: true,
                forecast: forecast(&long, &config),
            },
            RouteLeg {
                is_long: false,
                forecast: forecast(&short, &config),
            },
            Decimal::from(100_000),
            &CostModel::default(),
        )
    }

    #[test]
    fn transition_table_permits_open_path_only() {
        assert!(PositionState::Qualified.can_transition(PositionState::Reserved));
        assert!(PositionState::Reserved.can_transition(PositionState::Opening));
        assert!(PositionState::Opening.can_transition(PositionState::Open));
        assert!(PositionState::Opening.can_transition(PositionState::OpenFailed));
        // Illegal skips are rejected.
        assert!(!PositionState::Qualified.can_transition(PositionState::Open));
        assert!(!PositionState::Open.can_transition(PositionState::Opening));

        assert_eq!(
            PositionState::Opening.try_transition(PositionState::Open),
            Ok(PositionState::Open),
        );
        let rejected = PositionState::Qualified
            .try_transition(PositionState::Open)
            .expect_err("illegal edge is rejected");
        assert_eq!(rejected.from, PositionState::Qualified);
        assert_eq!(rejected.to, PositionState::Open);
    }

    #[test]
    fn open_fills_both_legs_and_stays_within_residual_cap() {
        let opp = opportunity();
        let long_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_000),
        };
        let short_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_030),
        };
        let position = open_paper(
            "0190abcd-1111-7000-8000-000000000001".into(),
            "key-1".into(),
            &opp,
            &long_exec,
            &short_exec,
            Decimal::from(100_000),
            Decimal::from(100_000),
            &CostModel::default(),
            &PaperFillConfig::default(),
            &RiskLimits::default(),
            Utc::now(),
        );
        assert_eq!(position.state, PositionState::Open);
        assert!(position.long_leg.filled_qty > Decimal::ZERO);
        assert!(position.short_leg.filled_qty > Decimal::ZERO);
        assert_eq!(position.long_leg.orders.len(), 1);
        assert_eq!(position.long_leg.fills.len(), 1);
        assert!(position.residual_delta_usd <= RiskLimits::default().max_unmatched());
        assert!(
            position
                .long_leg
                .client_ids_start_with("fa-0190abcd-open-long")
        );
    }

    #[test]
    fn oversized_residual_fails_open() {
        let opp = opportunity();
        // A coarse lot step plus mismatched marks leaves a large unmatched leg.
        let long_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_000),
        };
        let short_exec = LegExecution {
            symbol: "ETHUSDT".into(),
            mark_price: Decimal::from(45_000),
        };
        let position = open_paper(
            "0190abcd-2222-7000-8000-000000000002".into(),
            "key-2".into(),
            &opp,
            &long_exec,
            &short_exec,
            Decimal::from(100_000),
            Decimal::from(100_000),
            &CostModel::default(),
            &PaperFillConfig {
                lot_step: Decimal::ONE,
            },
            &RiskLimits::default(),
            Utc::now(),
        );
        assert_eq!(position.state, PositionState::OpenFailed);
        assert!(position.residual_delta_usd > RiskLimits::default().max_unmatched());
    }

    #[test]
    fn open_is_deterministic_for_identical_input() {
        let opp = opportunity();
        let long_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_000),
        };
        let short_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_030),
        };
        let build = || {
            open_paper(
                "0190abcd-3333-7000-8000-000000000003".into(),
                "key-3".into(),
                &opp,
                &long_exec,
                &short_exec,
                Decimal::from(100_000),
                Decimal::from(100_000),
                &CostModel::default(),
                &PaperFillConfig::default(),
                &RiskLimits::default(),
                Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap(),
            )
        };
        let first = build();
        let second = build();
        assert_eq!(first.long_leg.filled_qty, second.long_leg.filled_qty);
        assert_eq!(
            first.short_leg.avg_fill_price,
            second.short_leg.avg_fill_price
        );
        assert_eq!(first.residual_delta_usd, second.residual_delta_usd);
    }

    impl super::PositionLeg {
        fn client_ids_start_with(&self, prefix: &str) -> bool {
            self.orders
                .iter()
                .all(|order| order.client_order_id.starts_with(prefix))
        }
    }
}
