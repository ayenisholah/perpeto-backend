//! Deterministic market fixtures for the paper scanner (no network, no creds).
//!
//! The snapshot is embedded at compile time and normalized into domain
//! `FundingObservation`s so a scan is fully reproducible. Venue connectors (M3)
//! will replace this loader with live, credentialed feeds.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use funding_arb_domain::market::{FundingObservation, InstrumentRef, Venue};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Deserialize;

const MARKETS_JSON: &str = include_str!("../fixtures/markets.json");

/// Whether a fixture leg is a spot or a perpetual.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstrumentKind {
    /// A spot instrument (pays no funding).
    Spot,
    /// A perpetual future.
    Perp,
}

/// A parsed fixture leg: its kind plus the normalized observation.
#[derive(Clone, Debug)]
pub struct MarketLeg {
    /// Whether the leg is spot or perpetual.
    pub kind: InstrumentKind,
    /// The normalized funding observation.
    pub observation: FundingObservation,
}

/// Raised when the embedded fixtures cannot be parsed.
#[derive(Clone, Debug)]
pub struct FixtureError(String);

impl fmt::Display for FixtureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "market fixture error: {}", self.0)
    }
}

impl std::error::Error for FixtureError {}

#[derive(Debug, Deserialize)]
struct FixtureFile {
    observations: Vec<FixtureObservation>,
}

#[derive(Debug, Deserialize)]
struct FixtureObservation {
    venue: String,
    symbol: String,
    underlying: String,
    kind: String,
    funding_rate: f64,
    interval_hours: u32,
    mark_price: f64,
    index_price: f64,
    available_notional: f64,
    observed_at: String,
    next_funding_at: String,
}

fn decimal(value: f64) -> Decimal {
    Decimal::from_f64(value).unwrap_or_default()
}

fn timestamp(value: &str) -> Result<DateTime<Utc>, FixtureError> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| FixtureError(error.to_string()))
}

/// Load and normalize the embedded market fixtures.
///
/// # Errors
/// Returns [`FixtureError`] if the JSON is malformed or contains an unknown
/// venue or instrument kind.
pub fn load() -> Result<Vec<MarketLeg>, FixtureError> {
    let file: FixtureFile =
        serde_json::from_str(MARKETS_JSON).map_err(|error| FixtureError(error.to_string()))?;
    let mut legs = Vec::with_capacity(file.observations.len());
    for entry in file.observations {
        let venue = Venue::from_str(&entry.venue)
            .map_err(|_| FixtureError(format!("unknown venue {}", entry.venue)))?;
        let kind = match entry.kind.to_ascii_uppercase().as_str() {
            "SPOT" => InstrumentKind::Spot,
            "PERP" => InstrumentKind::Perp,
            other => return Err(FixtureError(format!("unknown instrument kind {other}"))),
        };
        legs.push(MarketLeg {
            kind,
            observation: FundingObservation {
                instrument: InstrumentRef {
                    venue,
                    symbol: entry.symbol,
                    underlying: entry.underlying,
                },
                funding_rate: decimal(entry.funding_rate),
                interval_hours: entry.interval_hours,
                mark_price: decimal(entry.mark_price),
                index_price: decimal(entry.index_price),
                available_notional: decimal(entry.available_notional),
                observed_at: timestamp(&entry.observed_at)?,
                next_funding_at: timestamp(&entry.next_funding_at)?,
            },
        });
    }
    Ok(legs)
}
