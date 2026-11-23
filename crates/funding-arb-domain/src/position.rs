//! Paper position lifecycle: the coordinated open path (PRODUCT_SPEC §5).
//!
//! This module opens a **paper** arbitrage position from a ranked
//! [`Opportunity`]: it sizes both legs, simulates a deterministic paired taker
//! fill, measures residual (unmatched) delta, and gates it against the
//! configured risk cap. It is pure and network-free — the same lifecycle will
//! later drive live execution behind a connector. It also supervises open
//! positions with deterministic re-hedging, funding capture, and exit.
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

const REHEDGE_PERSISTENCE_TICKS: u32 = 2;

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
    /// An incomplete or unknown paired fill is being reconciled.
    Recovering,
    /// A corrective hedge is being filled.
    Rehedging,
    /// Both legs are being flattened.
    Closing,
    /// The position is fully flattened.
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
            Self::Recovering => "RECOVERING",
            Self::Rehedging => "REHEDGING",
            Self::Closing => "CLOSING",
            Self::Closed => "CLOSED",
        }
    }

    /// Parse the canonical wire spelling.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        Some(match value {
            "QUALIFIED" => Self::Qualified,
            "RESERVED" => Self::Reserved,
            "OPENING" => Self::Opening,
            "OPEN" => Self::Open,
            "OPEN_FAILED" => Self::OpenFailed,
            "RECOVERING" => Self::Recovering,
            "REHEDGING" => Self::Rehedging,
            "CLOSING" => Self::Closing,
            "CLOSED" => Self::Closed,
            _ => return None,
        })
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
                | (Self::Opening, Self::Recovering)
                | (Self::OpenFailed, Self::Recovering)
                | (Self::Recovering, Self::Open)
                | (Self::Recovering, Self::Closed)
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

/// Why a position was exited (PRODUCT_SPEC §5.8).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitReason {
    /// Route funding flipped so net funding is no longer positive.
    FundingFlip,
    /// Expected net return fell below the hold threshold.
    ThresholdBreach,
    /// A leg's venue became unhealthy.
    VenueUnhealthy,
    /// An operator closed the position.
    Manual,
    /// Opening recovery could not safely complete the pair and flattened it.
    RecoveryFailure,
    /// A circuit breaker required the position to be flattened.
    CircuitBreaker,
}

impl ExitReason {
    /// The canonical wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FundingFlip => "FUNDING_FLIP",
            Self::ThresholdBreach => "THRESHOLD_BREACH",
            Self::VenueUnhealthy => "VENUE_UNHEALTHY",
            Self::Manual => "MANUAL",
            Self::RecoveryFailure => "RECOVERY_FAILURE",
            Self::CircuitBreaker => "CIRCUIT_BREAKER",
        }
    }

    /// Parse the canonical wire spelling.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "FUNDING_FLIP" => Some(Self::FundingFlip),
            "THRESHOLD_BREACH" => Some(Self::ThresholdBreach),
            "VENUE_UNHEALTHY" => Some(Self::VenueUnhealthy),
            "MANUAL" => Some(Self::Manual),
            "RECOVERY_FAILURE" => Some(Self::RecoveryFailure),
            "CIRCUIT_BREAKER" => Some(Self::CircuitBreaker),
            _ => None,
        }
    }
}

/// The outcome of an exit evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitDecision {
    /// Keep the position open.
    Hold,
    /// Exit now for the given reason.
    Exit(ExitReason),
}

/// The executable snapshot of one leg at open time (from the ranked scan).
#[derive(Clone, Debug)]
pub struct LegExecution {
    /// The venue-native symbol, e.g. `BTCUSDT`.
    pub symbol: String,
    /// The mark price used to size and fill the leg.
    pub mark_price: Decimal,
    /// The normalized per-interval funding rate (positive ⇒ longs pay shorts).
    pub funding_rate: Decimal,
    /// The funding interval length, in hours.
    pub funding_interval_hours: u32,
    /// The executable price from walking a captured order book (DEC-0016).
    /// `None` falls back to the mark price crossed by the modeled slippage,
    /// which is how fixture snapshots, which carry no books, are filled.
    pub fill_price: Option<Decimal>,
}

/// Deterministic paper-fill parameters.
#[derive(Clone, Copy, Debug)]
pub struct PaperFillConfig {
    /// The base-quantity lot step each leg is rounded down to.
    pub lot_step: Decimal,
}

/// Deterministic result returned by one paper leg submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FillOutcome {
    /// The requested quantity is authoritative and fully filled.
    Full,
    /// Only the supplied fraction is authoritatively filled.
    Partial(Decimal),
    /// Submission was ambiguous; reconciliation authoritatively finds this fraction.
    Unknown(Decimal),
}

impl FillOutcome {
    fn fraction(self) -> Decimal {
        match self {
            Self::Full => Decimal::ONE,
            Self::Partial(value) | Self::Unknown(value) => value.clamp(Decimal::ZERO, Decimal::ONE),
        }
    }

    fn requires_recovery(self) -> bool {
        !matches!(self, Self::Full)
    }
}

/// Per-leg deterministic fill scenario for a paired paper open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaperFillScenario {
    /// Long-leg submission result.
    pub long: FillOutcome,
    /// Short-leg submission result.
    pub short: FillOutcome,
}

impl Default for PaperFillScenario {
    fn default() -> Self {
        Self {
            long: FillOutcome::Full,
            short: FillOutcome::Full,
        }
    }
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
    /// The mark price the leg was sized and filled against.
    pub mark_price: Decimal,
    /// The normalized per-interval funding rate for the leg's instrument.
    pub funding_rate: Decimal,
    /// The funding interval length, in hours.
    pub funding_interval_hours: u32,
    /// The base quantity targeted.
    pub target_qty: Decimal,
    /// The base quantity filled (zero once the leg is closed).
    pub filled_qty: Decimal,
    /// The average entry fill price.
    pub avg_fill_price: Decimal,
    /// Total fees paid on the leg over its lifetime (USD).
    pub fees: Decimal,
    /// The order intents recorded for the leg (open, then close).
    pub orders: Vec<PaperOrder>,
    /// The simulated fills for the leg (open, then close).
    pub fills: Vec<PaperFill>,
}

/// A paper arbitrage position produced by [`open_paper`].
#[derive(Clone, Debug)]
pub struct ArbitragePosition {
    /// A UUID string identifying the position.
    pub id: String,
    /// The route the position was opened from.
    pub route_id: String,
    /// Strategy that originated this position, if automatically opened.
    pub strategy_id: Option<String>,
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
    /// Unmatched base-asset exposure valued in USD (re-measured on supervision).
    pub residual_delta_usd: Decimal,
    /// Consecutive supervision ticks the residual cap has been breached.
    pub residual_breach_ticks: u32,
    /// Why the position was exited, once it has been.
    pub exit_reason: Option<ExitReason>,
    /// Net funding captured over the hold (USD); zero until close.
    pub funding_captured: Decimal,
    /// Realized PnL at close (funding + trading PnL − fees); zero until close.
    pub realized_pnl: Decimal,
    /// When the position was opened.
    pub opened_at: DateTime<Utc>,
    /// When the position was closed, if it has been.
    pub closed_at: Option<DateTime<Utc>>,
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
    outcome: FillOutcome,
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
    let price = exec.fill_price.unwrap_or(exec.mark_price * slip);
    let filled_qty = round_to_step(target_qty * outcome.fraction(), fill.lot_step);
    let fee = filled_qty * price * cost.taker_fee;
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
        qty: filled_qty,
        price,
        fee,
    };
    PositionLeg {
        venue,
        symbol: exec.symbol.clone(),
        is_long,
        mark_price: exec.mark_price,
        funding_rate: exec.funding_rate,
        funding_interval_hours: exec.funding_interval_hours,
        target_qty,
        filled_qty,
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
    open_paper_with_scenario(
        id,
        idempotency_key,
        opportunity,
        long_exec,
        short_exec,
        requested_notional,
        reserved_capital,
        cost,
        fill,
        limits,
        PaperFillScenario::default(),
        now,
    )
}

/// Open with deterministic per-leg partial or unknown outcomes and recover the
/// authoritative unmatched quantity before returning.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn open_paper_with_scenario(
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
    scenario: PaperFillScenario,
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
        scenario.long,
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
        scenario.short,
    );

    let residual_base = long_leg.filled_qty - short_leg.filled_qty;
    let reference_price = (long_exec.mark_price + short_exec.mark_price) / Decimal::from(2);
    let residual_delta_usd = abs(residual_base) * reference_price;

    // Any partial/unknown submission is reconciled before the position can open.
    let state = if scenario.long.requires_recovery() || scenario.short.requires_recovery() {
        PositionState::Recovering
    } else if residual_delta_usd <= limits.max_unmatched() {
        PositionState::Open
    } else {
        PositionState::OpenFailed
    };

    let position = ArbitragePosition {
        id,
        route_id: opportunity.route_id.clone(),
        strategy_id: None,
        underlying: opportunity.underlying.clone(),
        route_type: opportunity.route_type,
        funding_direction: opportunity.funding_direction,
        state,
        long_leg,
        short_leg,
        target_notional: requested_notional,
        reserved_capital,
        residual_delta_usd,
        residual_breach_ticks: 0,
        exit_reason: None,
        funding_captured: Decimal::ZERO,
        realized_pnl: Decimal::ZERO,
        opened_at: now,
        closed_at: None,
        idempotency_key,
    };
    recover_opening(
        position,
        long_exec.mark_price,
        short_exec.mark_price,
        limits,
        cost,
        fill,
    )
}

fn recovery_slippage_cap(underlying: &str) -> Decimal {
    if matches!(underlying, "BTC" | "ETH") {
        Decimal::new(10, 4)
    } else {
        Decimal::new(25, 4)
    }
}

/// Reconcile an incomplete paired open, complete the smaller leg inside the
/// configured slippage-loss cap, or flatten when the cap would be exceeded.
#[must_use]
pub fn recover_opening(
    mut position: ArbitragePosition,
    long_mark: Decimal,
    short_mark: Decimal,
    limits: &RiskLimits,
    cost: &CostModel,
    fill: &PaperFillConfig,
) -> ArbitragePosition {
    if position.state != PositionState::Recovering {
        return position;
    }
    let net = net_delta_usd(
        position.long_leg.filled_qty,
        long_mark,
        position.short_leg.filled_qty,
        short_mark,
    );
    let unmatched = abs(net);
    let loss = unmatched * cost.slippage;
    let loss_cap = unmatched * recovery_slippage_cap(&position.underlying);
    if loss <= loss_cap {
        let position_short: String = position.id.chars().take(8).collect();
        if net.is_sign_positive() && !short_mark.is_zero() {
            let quantity = round_to_step(net / short_mark, fill.lot_step);
            if quantity > Decimal::ZERO {
                position.short_leg = append_hedge_fill(
                    &position_short,
                    "short",
                    position.short_leg,
                    short_mark,
                    quantity,
                    cost,
                );
            }
        } else if net.is_sign_negative() && !long_mark.is_zero() {
            let quantity = round_to_step(abs(net) / long_mark, fill.lot_step);
            if quantity > Decimal::ZERO {
                position.long_leg = append_hedge_fill(
                    &position_short,
                    "long",
                    position.long_leg,
                    long_mark,
                    quantity,
                    cost,
                );
            }
        }
        position.residual_delta_usd = abs(net_delta_usd(
            position.long_leg.filled_qty,
            long_mark,
            position.short_leg.filled_qty,
            short_mark,
        ));
        if position.residual_delta_usd <= limits.max_unmatched() {
            position.state = PositionState::Open;
            return position;
        }
    }

    let position_short: String = position.id.chars().take(8).collect();
    let (long_leg, long_pnl) = close_leg(&position_short, "long", position.long_leg, cost);
    let (short_leg, short_pnl) = close_leg(&position_short, "short", position.short_leg, cost);
    position.long_leg = long_leg;
    position.short_leg = short_leg;
    position.realized_pnl = long_pnl + short_pnl - position.long_leg.fees - position.short_leg.fees;
    position.residual_delta_usd = Decimal::ZERO;
    position.exit_reason = Some(ExitReason::RecoveryFailure);
    position.closed_at = Some(position.opened_at);
    position.state = PositionState::Closed;
    position
}

/// Deterministic funding capture (PRODUCT_SPEC §3.3).
///
/// For each leg, the whole funding intervals elapsed since open are multiplied
/// by the per-interval cashflow `−position_sign × leg_notional × funding_rate`.
/// The short leg earns positive funding (a credit) and the long leg pays it (a
/// debit); the net is returned in USD. Deterministic in `(position, now)`.
#[must_use]
pub fn settle_funding(position: &ArbitragePosition, now: DateTime<Utc>) -> Decimal {
    let elapsed_hours = (now - position.opened_at).num_hours();
    leg_funding(&position.long_leg, elapsed_hours) + leg_funding(&position.short_leg, elapsed_hours)
}

/// Signed two-leg delta at the supplied current marks (PRODUCT_SPEC §3.3).
///
/// Positive values mean the long leg is larger in USD terms; negative values
/// mean the short leg is larger.
#[must_use]
pub fn net_delta_usd(
    long_qty: Decimal,
    long_mark: Decimal,
    short_qty: Decimal,
    short_mark: Decimal,
) -> Decimal {
    long_qty * long_mark - short_qty * short_mark
}

fn append_hedge_fill(
    position_short: &str,
    label: &str,
    mut leg: PositionLeg,
    mark: Decimal,
    quantity: Decimal,
    cost: &CostModel,
) -> PositionLeg {
    let slip = if leg.is_long {
        Decimal::ONE + cost.slippage
    } else {
        Decimal::ONE - cost.slippage
    };
    let fill_price = mark * slip;
    let fee = quantity * fill_price * cost.taker_fee;
    let hedge_marker = format!("-hedge-{label}-");
    let attempt = leg
        .orders
        .iter()
        .filter(|order| order.client_order_id.contains(&hedge_marker))
        .count()
        + 1;
    let client_order_id = format!("fa-{position_short}-hedge-{label}-{attempt}");
    leg.orders.push(PaperOrder {
        client_order_id: client_order_id.clone(),
        side: if leg.is_long { "BUY" } else { "SELL" },
        target_qty: quantity,
        price_cap: fill_price,
        time_in_force: "IOC",
    });
    leg.fills.push(PaperFill {
        client_order_id,
        qty: quantity,
        price: fill_price,
        fee,
    });
    let previous_qty = leg.filled_qty;
    let combined_qty = previous_qty + quantity;
    if !combined_qty.is_zero() {
        leg.avg_fill_price =
            (previous_qty * leg.avg_fill_price + quantity * fill_price) / combined_qty;
    }
    leg.filled_qty = combined_qty;
    leg.fees += fee;
    leg
}

/// Re-measure and, after two consecutive breaches, correct a paper position.
///
/// Slice 5a deliberately uses the existing unmatched-exposure limit and a
/// two-tick persistence rule so the deterministic market path can exercise the
/// supervisor. Production time/bps thresholds remain a later risk-control
/// concern.
#[must_use]
pub fn re_hedge(
    mut position: ArbitragePosition,
    long_mark_now: Decimal,
    short_mark_now: Decimal,
    limits: &RiskLimits,
    cost: &CostModel,
) -> ArbitragePosition {
    if position.state != PositionState::Open {
        return position;
    }
    position.long_leg.mark_price = long_mark_now;
    position.short_leg.mark_price = short_mark_now;
    let net = net_delta_usd(
        position.long_leg.filled_qty,
        long_mark_now,
        position.short_leg.filled_qty,
        short_mark_now,
    );
    position.residual_delta_usd = abs(net);
    if position.residual_delta_usd <= limits.max_unmatched() {
        position.residual_breach_ticks = 0;
        return position;
    }

    position.residual_breach_ticks = position.residual_breach_ticks.saturating_add(1);
    if position.residual_breach_ticks < REHEDGE_PERSISTENCE_TICKS {
        return position;
    }

    position.state = position
        .state
        .try_transition(PositionState::Rehedging)
        .unwrap_or(PositionState::Rehedging);
    let position_short: String = position.id.chars().take(8).collect();
    let lot_step = PaperFillConfig::default().lot_step;
    let mut corrected = false;
    if net.is_sign_positive() && !short_mark_now.is_zero() {
        let quantity = round_to_step(net / short_mark_now, lot_step);
        if quantity > Decimal::ZERO {
            position.short_leg = append_hedge_fill(
                &position_short,
                "short",
                position.short_leg,
                short_mark_now,
                quantity,
                cost,
            );
            corrected = true;
        }
    } else if net.is_sign_negative() && !long_mark_now.is_zero() {
        let quantity = round_to_step(abs(net) / long_mark_now, lot_step);
        if quantity > Decimal::ZERO {
            position.long_leg = append_hedge_fill(
                &position_short,
                "long",
                position.long_leg,
                long_mark_now,
                quantity,
                cost,
            );
            corrected = true;
        }
    }
    position.residual_delta_usd = abs(net_delta_usd(
        position.long_leg.filled_qty,
        long_mark_now,
        position.short_leg.filled_qty,
        short_mark_now,
    ));
    if corrected {
        position.residual_breach_ticks = 0;
    }
    position.state = position
        .state
        .try_transition(PositionState::Open)
        .unwrap_or(PositionState::Open);
    position
}

/// Decide whether current forecasts and venue health require an exit.
#[must_use]
pub fn evaluate_exit(
    net_funding_apr_now: Decimal,
    venue_healthy: bool,
    limits: &RiskLimits,
) -> ExitDecision {
    if !venue_healthy {
        ExitDecision::Exit(ExitReason::VenueUnhealthy)
    } else if net_funding_apr_now <= Decimal::ZERO {
        ExitDecision::Exit(ExitReason::FundingFlip)
    } else if net_funding_apr_now < limits.min_net_apr / Decimal::from(2) {
        ExitDecision::Exit(ExitReason::ThresholdBreach)
    } else {
        ExitDecision::Hold
    }
}

fn leg_funding(leg: &PositionLeg, elapsed_hours: i64) -> Decimal {
    if leg.funding_interval_hours == 0 || elapsed_hours <= 0 {
        return Decimal::ZERO;
    }
    let intervals = elapsed_hours / i64::from(leg.funding_interval_hours);
    let notional = leg.filled_qty * leg.avg_fill_price;
    let position_sign = if leg.is_long {
        Decimal::ONE
    } else {
        -Decimal::ONE
    };
    -position_sign * notional * leg.funding_rate * Decimal::from(intervals)
}

fn close_leg(
    position_short: &str,
    label: &str,
    mut leg: PositionLeg,
    cost: &CostModel,
) -> (PositionLeg, Decimal) {
    // Closing reverses the open side: a long leg sells, a short leg buys.
    let slip = if leg.is_long {
        Decimal::ONE - cost.slippage
    } else {
        Decimal::ONE + cost.slippage
    };
    let close_price = leg.mark_price * slip;
    let qty = leg.filled_qty;
    let exit_fee = qty * close_price * cost.taker_fee;
    let client_order_id = format!("fa-{position_short}-close-{label}-1");
    let order = PaperOrder {
        client_order_id: client_order_id.clone(),
        side: if leg.is_long { "SELL" } else { "BUY" },
        target_qty: qty,
        price_cap: close_price,
        time_in_force: "IOC",
    };
    let fill = PaperFill {
        client_order_id,
        qty,
        price: close_price,
        fee: exit_fee,
    };
    // Trading PnL from entry vs close fill prices (slippage is realized here;
    // taker fees are tracked separately in `fees`).
    let trading_pnl = if leg.is_long {
        (close_price - leg.avg_fill_price) * qty
    } else {
        (leg.avg_fill_price - close_price) * qty
    };
    leg.orders.push(order);
    leg.fills.push(fill);
    leg.fees += exit_fee;
    leg.filled_qty = Decimal::ZERO;
    (leg, trading_pnl)
}

/// Close a paper position: capture funding, flatten both legs with reduce-only /
/// sell-owned fills, and record realized PnL plus the supplied exit reason.
/// Only an `Open` position closes; any other state (including an already-`Closed`
/// one) is returned unchanged, so close is idempotent.
#[must_use]
pub fn close_paper(
    mut position: ArbitragePosition,
    now: DateTime<Utc>,
    cost: &CostModel,
    exit_reason: ExitReason,
) -> ArbitragePosition {
    if position.state != PositionState::Open {
        return position;
    }
    let funding_captured = settle_funding(&position, now);
    let position_short: String = position.id.chars().take(8).collect();
    let (long_leg, long_pnl) = close_leg(&position_short, "long", position.long_leg, cost);
    let (short_leg, short_pnl) = close_leg(&position_short, "short", position.short_leg, cost);
    let total_fees = long_leg.fees + short_leg.fees;
    position.long_leg = long_leg;
    position.short_leg = short_leg;
    position.funding_captured = funding_captured;
    position.realized_pnl = funding_captured + long_pnl + short_pnl - total_fees;
    position.exit_reason = Some(exit_reason);
    position.closed_at = Some(now);
    position.state = PositionState::Open
        .try_transition(PositionState::Closing)
        .and_then(|state| state.try_transition(PositionState::Closed))
        .unwrap_or(PositionState::Closed);
    position
}

#[cfg(test)]
mod tests {
    use super::{
        ExitDecision, ExitReason, FillOutcome, LegExecution, PaperFillConfig, PaperFillScenario,
        PositionState, close_paper, evaluate_exit, net_delta_usd, open_paper,
        open_paper_with_scenario, re_hedge, settle_funding,
    };
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
        assert!(PositionState::Opening.can_transition(PositionState::Recovering));
        assert!(PositionState::Recovering.can_transition(PositionState::Open));
        assert!(PositionState::Recovering.can_transition(PositionState::Closed));
        assert!(PositionState::Open.can_transition(PositionState::Rehedging));
        assert!(PositionState::Rehedging.can_transition(PositionState::Open));
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
    fn unknown_fill_is_reconciled_and_hedged_before_open() {
        let opp = opportunity();
        let long_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_000),
            funding_rate: Decimal::new(1, 4),
            funding_interval_hours: 8,
            fill_price: None,
        };
        let short_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_030),
            funding_rate: Decimal::new(30, 4),
            funding_interval_hours: 8,
            fill_price: None,
        };
        let position = open_paper_with_scenario(
            "0190abcd-4444-7000-8000-000000000004".into(),
            "recover-unknown".into(),
            &opp,
            &long_exec,
            &short_exec,
            Decimal::from(100_000),
            Decimal::from(100_000),
            &CostModel::default(),
            &PaperFillConfig::default(),
            &RiskLimits::default(),
            PaperFillScenario {
                long: FillOutcome::Unknown(Decimal::new(5, 1)),
                short: FillOutcome::Full,
            },
            Utc::now(),
        );
        assert_eq!(position.state, PositionState::Open);
        assert!(
            position
                .long_leg
                .orders
                .iter()
                .any(|order| order.client_order_id.contains("-hedge-long-"))
        );
        assert!(position.residual_delta_usd <= RiskLimits::default().max_unmatched());
    }

    #[test]
    fn partial_fill_flattens_when_recovery_loss_exceeds_cap() {
        let opp = opportunity();
        let long_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_000),
            funding_rate: Decimal::new(1, 4),
            funding_interval_hours: 8,
            fill_price: None,
        };
        let short_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_030),
            funding_rate: Decimal::new(30, 4),
            funding_interval_hours: 8,
            fill_price: None,
        };
        let cost = CostModel {
            slippage: Decimal::new(2, 2),
            ..CostModel::default()
        };
        let position = open_paper_with_scenario(
            "0190abcd-5555-7000-8000-000000000005".into(),
            "recover-flatten".into(),
            &opp,
            &long_exec,
            &short_exec,
            Decimal::from(100_000),
            Decimal::from(100_000),
            &cost,
            &PaperFillConfig::default(),
            &RiskLimits::default(),
            PaperFillScenario {
                long: FillOutcome::Partial(Decimal::new(5, 1)),
                short: FillOutcome::Full,
            },
            Utc::now(),
        );
        assert_eq!(position.state, PositionState::Closed);
        assert_eq!(position.exit_reason, Some(ExitReason::RecoveryFailure));
        assert_eq!(position.long_leg.filled_qty, Decimal::ZERO);
        assert_eq!(position.short_leg.filled_qty, Decimal::ZERO);
    }

    #[test]
    fn open_fills_both_legs_and_stays_within_residual_cap() {
        let opp = opportunity();
        let long_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_000),
            funding_rate: Decimal::new(1, 4),
            funding_interval_hours: 8,
            fill_price: None,
        };
        let short_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_030),
            funding_rate: Decimal::new(30, 4),
            funding_interval_hours: 8,
            fill_price: None,
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
            funding_rate: Decimal::new(1, 4),
            funding_interval_hours: 8,
            fill_price: None,
        };
        let short_exec = LegExecution {
            symbol: "ETHUSDT".into(),
            mark_price: Decimal::from(45_000),
            funding_rate: Decimal::new(30, 4),
            funding_interval_hours: 8,
            fill_price: None,
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
    fn a_book_fill_price_replaces_the_modeled_slippage() {
        let opp = opportunity();
        let at = Utc.with_ymd_and_hms(2026, 11, 22, 0, 0, 0).unwrap();
        let open = |long_fill: Option<Decimal>, short_fill: Option<Decimal>| {
            open_paper(
                "0190abcd-4444-7000-8000-000000000004".into(),
                "key-4".into(),
                &opp,
                &LegExecution {
                    fill_price: long_fill,
                    ..exec("BTCUSDT", 30_000, Decimal::new(1, 4))
                },
                &LegExecution {
                    fill_price: short_fill,
                    ..exec("BTCUSDT", 30_030, Decimal::new(30, 4))
                },
                Decimal::from(100_000),
                Decimal::from(100_000),
                &CostModel::default(),
                &PaperFillConfig::default(),
                &RiskLimits::default(),
                at,
            )
        };
        let modeled = open(None, None);
        let slippage = CostModel::default().slippage;
        assert_eq!(
            modeled.long_leg.fills[0].price,
            Decimal::from(30_000) * (Decimal::ONE + slippage)
        );

        let booked = open(
            Some(Decimal::new(300_123, 1)),
            Some(Decimal::new(300_105, 1)),
        );
        assert_eq!(booked.long_leg.fills[0].price, Decimal::new(300_123, 1));
        assert_eq!(booked.short_leg.fills[0].price, Decimal::new(300_105, 1));
        // Sizing still uses the mark, so quantities match the modeled open.
        assert_eq!(booked.long_leg.target_qty, modeled.long_leg.target_qty);
    }

    #[test]
    fn open_is_deterministic_for_identical_input() {
        let opp = opportunity();
        let long_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_000),
            funding_rate: Decimal::new(1, 4),
            funding_interval_hours: 8,
            fill_price: None,
        };
        let short_exec = LegExecution {
            symbol: "BTCUSDT".into(),
            mark_price: Decimal::from(30_030),
            funding_rate: Decimal::new(30, 4),
            funding_interval_hours: 8,
            fill_price: None,
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

    fn exec(symbol: &str, mark: i64, rate: Decimal) -> LegExecution {
        LegExecution {
            symbol: symbol.into(),
            mark_price: Decimal::from(mark),
            funding_rate: rate,
            funding_interval_hours: 8,
            fill_price: None,
        }
    }

    fn open_at(id: &str, opened: chrono::DateTime<Utc>) -> super::ArbitragePosition {
        let opp = opportunity();
        open_paper(
            id.into(),
            "key".into(),
            &opp,
            &exec("BTCUSDT", 30_000, Decimal::new(1, 4)),
            &exec("BTCUSDT", 30_030, Decimal::new(30, 4)),
            Decimal::from(100_000),
            Decimal::from(100_000),
            &CostModel::default(),
            &PaperFillConfig::default(),
            &RiskLimits::default(),
            opened,
        )
    }

    #[test]
    fn funding_capture_scales_with_intervals_and_is_net_positive() {
        let opened = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let position = open_at("0190abcd-4444-7000-8000-000000000004", opened);
        let one = settle_funding(&position, opened + chrono::Duration::hours(8));
        let three = settle_funding(&position, opened + chrono::Duration::hours(24));
        // Short earns 0.30%/interval, long pays 0.01%, so net capture is positive.
        assert!(one > Decimal::ZERO);
        assert_eq!(three, one * Decimal::from(3));
        // Nothing settles before the first full interval.
        assert_eq!(
            settle_funding(&position, opened + chrono::Duration::hours(7)),
            Decimal::ZERO,
        );
    }

    #[test]
    fn exit_reason_wire_values_round_trip() {
        for reason in [
            ExitReason::FundingFlip,
            ExitReason::ThresholdBreach,
            ExitReason::VenueUnhealthy,
            ExitReason::Manual,
        ] {
            assert_eq!(ExitReason::from_wire(reason.as_str()), Some(reason));
        }
        assert_eq!(ExitReason::from_wire("UNKNOWN"), None);
    }

    #[test]
    fn net_delta_is_signed_and_exit_precedence_is_deterministic() {
        assert_eq!(
            net_delta_usd(
                Decimal::from(2),
                Decimal::from(100),
                Decimal::ONE,
                Decimal::from(150),
            ),
            Decimal::from(50),
        );
        let limits = RiskLimits::default();
        assert_eq!(
            evaluate_exit(-Decimal::ONE, false, &limits),
            ExitDecision::Exit(ExitReason::VenueUnhealthy),
        );
        assert_eq!(
            evaluate_exit(Decimal::ZERO, true, &limits),
            ExitDecision::Exit(ExitReason::FundingFlip),
        );
        assert_eq!(
            evaluate_exit(Decimal::new(7, 2), true, &limits),
            ExitDecision::Exit(ExitReason::ThresholdBreach),
        );
        assert_eq!(
            evaluate_exit(limits.min_net_apr / Decimal::from(2), true, &limits),
            ExitDecision::Hold,
        );
    }

    #[test]
    fn persistent_residual_rehedges_the_exposure_reducing_leg() {
        let opened = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let position = open_at("0190abcd-7777-7000-8000-000000000007", opened);
        let limits = RiskLimits::default();
        let cost = CostModel::default();

        let first = re_hedge(
            position,
            Decimal::from(30_000),
            Decimal::from(32_000),
            &limits,
            &cost,
        );
        assert_eq!(first.state, PositionState::Open);
        assert_eq!(first.residual_breach_ticks, 1);
        assert_eq!(first.long_leg.orders.len(), 1);
        let first_fees = first.long_leg.fees;

        let second = re_hedge(
            first.clone(),
            Decimal::from(30_000),
            Decimal::from(32_000),
            &limits,
            &cost,
        );
        let repeated = re_hedge(
            first,
            Decimal::from(30_000),
            Decimal::from(32_000),
            &limits,
            &cost,
        );
        assert_eq!(second.state, PositionState::Open);
        assert_eq!(second.residual_breach_ticks, 0);
        assert!(second.residual_delta_usd <= limits.max_unmatched());
        assert_eq!(second.long_leg.orders.len(), 2);
        assert!(
            second
                .long_leg
                .client_ids_contains("fa-0190abcd-hedge-long-1")
        );
        assert!(second.long_leg.fees > first_fees);
        assert_eq!(
            second.long_leg.orders[1].client_order_id,
            repeated.long_leg.orders[1].client_order_id,
        );
        assert_eq!(second.residual_delta_usd, repeated.residual_delta_usd);
    }

    #[test]
    fn residual_counter_resets_when_delta_returns_within_limit() {
        let opened = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let mut position = open_at("0190abcd-8888-7000-8000-000000000008", opened);
        position.residual_breach_ticks = 1;
        let long_mark = Decimal::from(30_000);
        let short_mark = position.long_leg.filled_qty * long_mark / position.short_leg.filled_qty;
        let supervised = re_hedge(
            position,
            long_mark,
            short_mark,
            &RiskLimits::default(),
            &CostModel::default(),
        );
        assert_eq!(supervised.residual_breach_ticks, 0);
        assert_eq!(supervised.residual_delta_usd, Decimal::ZERO);
    }

    #[test]
    fn close_flattens_legs_and_records_realized_pnl() {
        let opened = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let position = open_at("0190abcd-5555-7000-8000-000000000005", opened);
        let entry_fees = position.long_leg.fees + position.short_leg.fees;
        let closed = close_paper(
            position,
            opened + chrono::Duration::hours(24),
            &CostModel::default(),
            ExitReason::Manual,
        );

        assert_eq!(closed.state, PositionState::Closed);
        assert!(closed.closed_at.is_some());
        assert_eq!(closed.exit_reason, Some(ExitReason::Manual));
        assert!(closed.funding_captured > Decimal::ZERO);
        assert_eq!(closed.long_leg.filled_qty, Decimal::ZERO);
        assert_eq!(closed.short_leg.filled_qty, Decimal::ZERO);
        // Each leg gained a close order + fill on top of its open pair.
        assert_eq!(closed.long_leg.orders.len(), 2);
        assert_eq!(closed.short_leg.fills.len(), 2);
        assert!(
            closed
                .long_leg
                .client_ids_contains("fa-0190abcd-close-long")
        );
        // Exit fees were charged and realized PnL is funding net of all costs.
        assert!(closed.long_leg.fees + closed.short_leg.fees > entry_fees);
        assert!(closed.realized_pnl <= closed.funding_captured);
    }

    #[test]
    fn close_only_from_open_and_is_idempotent() {
        let opened = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let position = open_at("0190abcd-6666-7000-8000-000000000006", opened);
        let closed = close_paper(
            position,
            opened + chrono::Duration::hours(24),
            &CostModel::default(),
            ExitReason::Manual,
        );
        assert_eq!(closed.state, PositionState::Closed);
        // Closing again is a no-op: same state, funding, and order count.
        let long_orders = closed.long_leg.orders.len();
        let again = close_paper(
            closed.clone(),
            opened + chrono::Duration::hours(48),
            &CostModel::default(),
            ExitReason::FundingFlip,
        );
        assert_eq!(again.state, PositionState::Closed);
        assert_eq!(again.funding_captured, closed.funding_captured);
        assert_eq!(again.long_leg.orders.len(), long_orders);
    }

    impl super::PositionLeg {
        fn client_ids_start_with(&self, prefix: &str) -> bool {
            self.orders
                .iter()
                .all(|order| order.client_order_id.starts_with(prefix))
        }

        fn client_ids_contains(&self, prefix: &str) -> bool {
            self.orders
                .iter()
                .any(|order| order.client_order_id.starts_with(prefix))
        }
    }
}
