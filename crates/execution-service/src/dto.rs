//! Wire DTOs for paper positions (numbers rendered as plain JSON floats).
//!
//! `PositionDto` is served by the API and is also the serialized payload
//! persisted per position, so it derives `Deserialize` (mirroring the scanner's
//! `OpportunityDto`).

use funding_arb_domain::position::{ArbitragePosition, PaperFill, PaperOrder, PositionLeg};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

fn as_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

/// The request body for opening a paper position.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OpenPositionRequest {
    /// The route id from the latest scan to open.
    pub route_id: String,
    /// Optional notional (USD) to size each leg to; defaults to the reservation.
    #[serde(default)]
    pub notional: Option<f64>,
}

/// A recorded order intent, as served by the API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperOrderDto {
    /// Deterministic client order id.
    pub client_order_id: String,
    /// `BUY` or `SELL`.
    pub side: String,
    /// The base quantity requested.
    pub target_qty: f64,
    /// The price cap the IOC would not cross.
    pub price_cap: f64,
    /// Time in force (always `IOC` for the paired open).
    pub time_in_force: String,
}

/// A simulated fill, as served by the API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaperFillDto {
    /// The client order id the fill belongs to.
    pub client_order_id: String,
    /// The filled base quantity.
    pub qty: f64,
    /// The fill price after modeled slippage.
    pub price: f64,
    /// The taker fee paid (USD).
    pub fee: f64,
}

/// One leg of an opened paper position.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PositionLegDto {
    /// The leg's venue.
    pub venue: String,
    /// The venue-native symbol.
    pub symbol: String,
    /// Whether the leg is held long.
    pub is_long: bool,
    /// The base quantity targeted.
    pub target_qty: f64,
    /// The base quantity filled.
    pub filled_qty: f64,
    /// The average fill price.
    pub avg_fill_price: f64,
    /// Total fees on the leg (USD).
    pub fees: f64,
    /// The order intents recorded for the leg.
    pub orders: Vec<PaperOrderDto>,
    /// The simulated fills for the leg.
    pub fills: Vec<PaperFillDto>,
}

/// A paper arbitrage position, as served by the API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PositionDto {
    /// The position id (UUID string).
    pub id: String,
    /// The route the position was opened from.
    pub route_id: String,
    /// The underlying asset.
    pub underlying: String,
    /// `SPOT_PERP` or `PERP_PERP`.
    pub route_type: String,
    /// `POSITIVE` or `NEGATIVE` funding capture.
    pub funding_direction: String,
    /// The lifecycle state (`OPEN` or `OPEN_FAILED` in this slice).
    pub state: String,
    /// The requested notional (USD) sized per leg.
    pub target_notional: f64,
    /// Capital (USD) reserved for the position.
    pub reserved_capital: f64,
    /// Unmatched base-asset exposure valued in USD.
    pub residual_delta_usd: f64,
    /// RFC 3339 timestamp of when the position was opened.
    pub opened_at: String,
    /// The long leg.
    pub long_leg: PositionLegDto,
    /// The short leg.
    pub short_leg: PositionLegDto,
}

/// A list of paper positions, as served by the API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PositionListDto {
    /// The positions, newest first.
    pub positions: Vec<PositionDto>,
}

impl PaperOrderDto {
    fn from_domain(order: &PaperOrder) -> Self {
        Self {
            client_order_id: order.client_order_id.clone(),
            side: order.side.to_owned(),
            target_qty: as_f64(order.target_qty),
            price_cap: as_f64(order.price_cap),
            time_in_force: order.time_in_force.to_owned(),
        }
    }
}

impl PaperFillDto {
    fn from_domain(fill: &PaperFill) -> Self {
        Self {
            client_order_id: fill.client_order_id.clone(),
            qty: as_f64(fill.qty),
            price: as_f64(fill.price),
            fee: as_f64(fill.fee),
        }
    }
}

impl PositionLegDto {
    fn from_domain(leg: &PositionLeg) -> Self {
        Self {
            venue: leg.venue.as_str().to_owned(),
            symbol: leg.symbol.clone(),
            is_long: leg.is_long,
            target_qty: as_f64(leg.target_qty),
            filled_qty: as_f64(leg.filled_qty),
            avg_fill_price: as_f64(leg.avg_fill_price),
            fees: as_f64(leg.fees),
            orders: leg.orders.iter().map(PaperOrderDto::from_domain).collect(),
            fills: leg.fills.iter().map(PaperFillDto::from_domain).collect(),
        }
    }
}

impl PositionDto {
    /// Project a domain position onto the wire shape.
    #[must_use]
    pub fn from_domain(position: &ArbitragePosition) -> Self {
        Self {
            id: position.id.clone(),
            route_id: position.route_id.clone(),
            underlying: position.underlying.clone(),
            route_type: position.route_type.as_str().to_owned(),
            funding_direction: position.funding_direction.as_str().to_owned(),
            state: position.state.as_str().to_owned(),
            target_notional: as_f64(position.target_notional),
            reserved_capital: as_f64(position.reserved_capital),
            residual_delta_usd: as_f64(position.residual_delta_usd),
            opened_at: position.opened_at.to_rfc3339(),
            long_leg: PositionLegDto::from_domain(&position.long_leg),
            short_leg: PositionLegDto::from_domain(&position.short_leg),
        }
    }
}
