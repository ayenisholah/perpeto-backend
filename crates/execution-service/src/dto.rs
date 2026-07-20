//! Wire DTOs for paper positions (numbers rendered as plain JSON floats).
//!
//! `PositionDto` is served by the API and is also the serialized payload
//! persisted per position, so it derives `Deserialize` (mirroring the scanner's
//! `OpportunityDto`).

use std::str::FromStr;

use chrono::{DateTime, Utc};
use funding_arb_domain::market::{RouteType, Venue};
use funding_arb_domain::opportunity::FundingDirection;
use funding_arb_domain::position::{
    ArbitragePosition, ExitReason, PaperFill, PaperOrder, PositionLeg, PositionState,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

fn as_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

fn dec(value: f64) -> Decimal {
    Decimal::from_f64_retain(value).unwrap_or_default()
}

fn parse_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .unwrap_or_else(|_| DateTime::from_timestamp(0, 0).unwrap())
}

fn side_static(value: &str) -> &'static str {
    if value == "BUY" { "BUY" } else { "SELL" }
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
    /// The mark price the leg was sized and filled against.
    pub mark_price: f64,
    /// The normalized per-interval funding rate for the leg's instrument.
    pub funding_rate: f64,
    /// The funding interval length, in hours.
    pub funding_interval_hours: u32,
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
    /// The current lifecycle state.
    pub state: String,
    /// The requested notional (USD) sized per leg.
    pub target_notional: f64,
    /// Capital (USD) reserved for the position.
    pub reserved_capital: f64,
    /// Unmatched base-asset exposure valued in USD.
    pub residual_delta_usd: f64,
    /// Consecutive supervision ticks above the residual limit.
    #[serde(default)]
    pub residual_breach_ticks: u32,
    /// Why the position exited; explicitly null while it remains open.
    #[serde(default)]
    pub exit_reason: Option<String>,
    /// Net funding captured over the hold (USD); zero until close.
    pub funding_captured: f64,
    /// Realized PnL at close (USD); zero until close.
    pub realized_pnl: f64,
    /// RFC 3339 timestamp of when the position was opened.
    pub opened_at: String,
    /// RFC 3339 timestamp of when the position was closed; absent while open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
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

    fn to_domain(&self) -> PaperOrder {
        PaperOrder {
            client_order_id: self.client_order_id.clone(),
            side: side_static(&self.side),
            target_qty: dec(self.target_qty),
            price_cap: dec(self.price_cap),
            // v1 only issues IOC orders.
            time_in_force: "IOC",
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

    fn to_domain(&self) -> PaperFill {
        PaperFill {
            client_order_id: self.client_order_id.clone(),
            qty: dec(self.qty),
            price: dec(self.price),
            fee: dec(self.fee),
        }
    }
}

impl PositionLegDto {
    fn from_domain(leg: &PositionLeg) -> Self {
        Self {
            venue: leg.venue.as_str().to_owned(),
            symbol: leg.symbol.clone(),
            is_long: leg.is_long,
            mark_price: as_f64(leg.mark_price),
            funding_rate: as_f64(leg.funding_rate),
            funding_interval_hours: leg.funding_interval_hours,
            target_qty: as_f64(leg.target_qty),
            filled_qty: as_f64(leg.filled_qty),
            avg_fill_price: as_f64(leg.avg_fill_price),
            fees: as_f64(leg.fees),
            orders: leg.orders.iter().map(PaperOrderDto::from_domain).collect(),
            fills: leg.fills.iter().map(PaperFillDto::from_domain).collect(),
        }
    }

    fn to_domain(&self) -> PositionLeg {
        PositionLeg {
            venue: Venue::from_str(&self.venue).unwrap_or(Venue::Binance),
            symbol: self.symbol.clone(),
            is_long: self.is_long,
            mark_price: dec(self.mark_price),
            funding_rate: dec(self.funding_rate),
            funding_interval_hours: self.funding_interval_hours,
            target_qty: dec(self.target_qty),
            filled_qty: dec(self.filled_qty),
            avg_fill_price: dec(self.avg_fill_price),
            fees: dec(self.fees),
            orders: self.orders.iter().map(PaperOrderDto::to_domain).collect(),
            fills: self.fills.iter().map(PaperFillDto::to_domain).collect(),
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
            residual_breach_ticks: position.residual_breach_ticks,
            exit_reason: position
                .exit_reason
                .map(|reason| reason.as_str().to_owned()),
            funding_captured: as_f64(position.funding_captured),
            realized_pnl: as_f64(position.realized_pnl),
            opened_at: position.opened_at.to_rfc3339(),
            closed_at: position.closed_at.map(|at| at.to_rfc3339()),
            long_leg: PositionLegDto::from_domain(&position.long_leg),
            short_leg: PositionLegDto::from_domain(&position.short_leg),
        }
    }

    /// Reconstruct the domain position from its stored wire shape, so the close
    /// path can operate on it. (Values round-trip through `f64`; lossless decimal
    /// persistence is deferred to the ledger slice.)
    #[must_use]
    pub fn to_domain(&self) -> ArbitragePosition {
        ArbitragePosition {
            id: self.id.clone(),
            route_id: self.route_id.clone(),
            underlying: self.underlying.clone(),
            route_type: RouteType::from_str(&self.route_type).unwrap_or(RouteType::PerpPerp),
            funding_direction: FundingDirection::from_wire(&self.funding_direction)
                .unwrap_or(FundingDirection::Positive),
            state: PositionState::from_wire(&self.state).unwrap_or(PositionState::Open),
            long_leg: self.long_leg.to_domain(),
            short_leg: self.short_leg.to_domain(),
            target_notional: dec(self.target_notional),
            reserved_capital: dec(self.reserved_capital),
            residual_delta_usd: dec(self.residual_delta_usd),
            residual_breach_ticks: self.residual_breach_ticks,
            exit_reason: self.exit_reason.as_deref().and_then(ExitReason::from_wire),
            funding_captured: dec(self.funding_captured),
            realized_pnl: dec(self.realized_pnl),
            opened_at: parse_time(&self.opened_at),
            closed_at: self.closed_at.as_deref().map(parse_time),
            idempotency_key: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PositionDto;

    #[test]
    fn legacy_position_payload_defaults_supervision_fields() {
        let payload = r#"{
            "id":"0190abcd-1111-7000-8000-000000000001",
            "route_id":"PERP_PERP:BINANCE>BYBIT:BTC",
            "underlying":"BTC",
            "route_type":"PERP_PERP",
            "funding_direction":"POSITIVE",
            "state":"OPEN",
            "target_notional":100000.0,
            "reserved_capital":50000.0,
            "residual_delta_usd":10.0,
            "funding_captured":0.0,
            "realized_pnl":0.0,
            "opened_at":"2026-07-19T00:00:00Z",
            "long_leg":{"venue":"BINANCE","symbol":"BTCUSDT-PERP","is_long":true,"mark_price":30030.0,"funding_rate":0.0001,"funding_interval_hours":8,"target_qty":3.0,"filled_qty":3.0,"avg_fill_price":30039.0,"fees":45.0,"orders":[],"fills":[]},
            "short_leg":{"venue":"BYBIT","symbol":"BTCUSDT-PERP","is_long":false,"mark_price":30045.0,"funding_rate":0.0009,"funding_interval_hours":8,"target_qty":3.0,"filled_qty":3.0,"avg_fill_price":30036.0,"fees":45.0,"orders":[],"fills":[]}
        }"#;
        let dto: PositionDto = serde_json::from_str(payload).expect("legacy payload");
        assert_eq!(dto.residual_breach_ticks, 0);
        assert_eq!(dto.exit_reason, None);
        let domain = dto.to_domain();
        assert_eq!(domain.residual_breach_ticks, 0);
        assert_eq!(domain.exit_reason, None);
    }
}
