//! Normalized market reference types for the funding-rate scanner.
//!
//! These are transport- and venue-agnostic. Connectors (M3) map raw venue
//! payloads into these types; during M2 they are produced from deterministic
//! fixtures so the scanner is fully reproducible.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

/// A trading venue Perpeto can observe.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Venue {
    Binance,
    Bybit,
    Okx,
    DyDx,
    Hyperliquid,
}

impl Venue {
    /// The canonical wire spelling of the venue.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Binance => "BINANCE",
            Self::Bybit => "BYBIT",
            Self::Okx => "OKX",
            Self::DyDx => "DYDX",
            Self::Hyperliquid => "HYPERLIQUID",
        }
    }
}

impl fmt::Display for Venue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Returned when a venue string is not recognized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseVenueError;

impl fmt::Display for ParseVenueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unknown venue")
    }
}

impl std::error::Error for ParseVenueError {}

impl FromStr for Venue {
    type Err = ParseVenueError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_uppercase().as_str() {
            "BINANCE" => Ok(Self::Binance),
            "BYBIT" => Ok(Self::Bybit),
            "OKX" => Ok(Self::Okx),
            "DYDX" => Ok(Self::DyDx),
            "HYPERLIQUID" => Ok(Self::Hyperliquid),
            _ => Err(ParseVenueError),
        }
    }
}

/// Whether a route pairs a spot leg with a perpetual, or two perpetuals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RouteType {
    /// Long spot, short perpetual (or the mirror) on one underlying.
    SpotPerp,
    /// Long one perpetual, short another perpetual on the same underlying.
    PerpPerp,
}

impl RouteType {
    /// The canonical wire spelling of the route type.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpotPerp => "SPOT_PERP",
            Self::PerpPerp => "PERP_PERP",
        }
    }
}

impl fmt::Display for RouteType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Returned when a route-type string is not recognized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseRouteTypeError;

impl fmt::Display for ParseRouteTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unknown route type")
    }
}

impl std::error::Error for ParseRouteTypeError {}

impl FromStr for RouteType {
    type Err = ParseRouteTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_uppercase().as_str() {
            "SPOT_PERP" => Ok(Self::SpotPerp),
            "PERP_PERP" => Ok(Self::PerpPerp),
            _ => Err(ParseRouteTypeError),
        }
    }
}

/// A venue-scoped instrument identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstrumentRef {
    /// The venue the instrument trades on.
    pub venue: Venue,
    /// The venue-native symbol, e.g. `BTCUSDT`.
    pub symbol: String,
    /// The underlying asset, e.g. `BTC`.
    pub underlying: String,
}

/// A single funding observation for a perpetual instrument.
#[derive(Clone, Debug)]
pub struct FundingObservation {
    /// The instrument the observation belongs to.
    pub instrument: InstrumentRef,
    /// The realized/estimated funding rate for one interval (e.g. `0.0001`).
    pub funding_rate: Decimal,
    /// The funding interval length in hours (e.g. `8`).
    pub interval_hours: u32,
    /// The current mark price.
    pub mark_price: Decimal,
    /// The current index price.
    pub index_price: Decimal,
    /// Depth-limited executable notional in USD (the eligibility gate input).
    pub available_notional: Decimal,
    /// When the observation was taken.
    pub observed_at: DateTime<Utc>,
    /// When funding next settles.
    pub next_funding_at: DateTime<Utc>,
}

impl FundingObservation {
    /// The number of funding settlements per year for this interval.
    #[must_use]
    pub fn intervals_per_year(&self) -> Decimal {
        let hours = Decimal::from(self.interval_hours.max(1));
        (Decimal::from(24) / hours) * Decimal::from(365)
    }

    /// The annualized funding rate (APR) implied by this observation.
    #[must_use]
    pub fn annualized_rate(&self) -> Decimal {
        self.funding_rate * self.intervals_per_year()
    }

    /// The perpetual premium `(mark - index) / index`; zero when index is zero.
    #[must_use]
    pub fn premium(&self) -> Decimal {
        if self.index_price.is_zero() {
            Decimal::ZERO
        } else {
            (self.mark_price - self.index_price) / self.index_price
        }
    }
}
