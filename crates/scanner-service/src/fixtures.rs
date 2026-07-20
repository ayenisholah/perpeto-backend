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
const MARKET_PATH_JSON: &str = include_str!("../fixtures/market_path.json");

/// Whether a fixture leg is a spot or a perpetual.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstrumentKind {
    /// A spot instrument (pays no funding).
    Spot,
    /// A perpetual future.
    Perp,
}

/// Deterministic venue health attached to a fixture observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarketStatus {
    /// Venue data and paper execution are healthy.
    Healthy,
    /// Venue data is degraded and new exposure must be disabled.
    Degraded,
}

/// A parsed fixture leg: its kind plus the normalized observation.
#[derive(Clone, Debug)]
pub struct MarketLeg {
    /// Whether the leg is spot or perpetual.
    pub kind: InstrumentKind,
    /// Health of the venue at this fixture tick.
    pub status: MarketStatus,
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
struct PathFile {
    snapshots: Vec<PathSnapshot>,
}

#[derive(Debug, Deserialize)]
struct PathSnapshot {
    effective_at: String,
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
    #[serde(default = "healthy_status")]
    status: String,
}

fn healthy_status() -> String {
    "HEALTHY".to_owned()
}

fn decimal(value: f64) -> Decimal {
    Decimal::from_f64(value).unwrap_or_default()
}

fn timestamp(value: &str) -> Result<DateTime<Utc>, FixtureError> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| FixtureError(error.to_string()))
}

fn normalize(entries: Vec<FixtureObservation>) -> Result<Vec<MarketLeg>, FixtureError> {
    let mut legs = Vec::with_capacity(entries.len());
    for entry in entries {
        let venue = Venue::from_str(&entry.venue)
            .map_err(|_| FixtureError(format!("unknown venue {}", entry.venue)))?;
        let kind = match entry.kind.to_ascii_uppercase().as_str() {
            "SPOT" => InstrumentKind::Spot,
            "PERP" => InstrumentKind::Perp,
            other => return Err(FixtureError(format!("unknown instrument kind {other}"))),
        };
        let status = match entry.status.to_ascii_uppercase().as_str() {
            "HEALTHY" => MarketStatus::Healthy,
            "DEGRADED" => MarketStatus::Degraded,
            other => return Err(FixtureError(format!("unknown market status {other}"))),
        };
        legs.push(MarketLeg {
            kind,
            status,
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

/// Load and normalize the embedded baseline market fixtures (`t0`).
///
/// # Errors
/// Returns [`FixtureError`] if the JSON is malformed or contains an unknown
/// venue or instrument kind.
pub fn load() -> Result<Vec<MarketLeg>, FixtureError> {
    let file: FixtureFile =
        serde_json::from_str(MARKETS_JSON).map_err(|error| FixtureError(error.to_string()))?;
    normalize(file.observations)
}

/// Load the market snapshot effective at `now` from the deterministic scenario
/// path — the latest snapshot whose `effective_at` is at or before `now`, or the
/// first snapshot when `now` precedes them all. This is what makes re-hedge and
/// auto-exit demonstrable: marks and funding evolve across ticks.
///
/// # Errors
/// Returns [`FixtureError`] if the path JSON is malformed or empty, or contains
/// an unknown venue or instrument kind.
pub fn load_at(now: DateTime<Utc>) -> Result<Vec<MarketLeg>, FixtureError> {
    let file: PathFile =
        serde_json::from_str(MARKET_PATH_JSON).map_err(|error| FixtureError(error.to_string()))?;
    // The greatest snapshot effective at or before `now`; else the earliest.
    let mut effective: Option<(DateTime<Utc>, Vec<FixtureObservation>)> = None;
    let mut earliest: Option<(DateTime<Utc>, Vec<FixtureObservation>)> = None;
    for snapshot in file.snapshots {
        let at = timestamp(&snapshot.effective_at)?;
        if at <= now {
            if effective.as_ref().is_none_or(|(best, _)| at >= *best) {
                effective = Some((at, snapshot.observations));
            }
        } else if earliest.as_ref().is_none_or(|(first, _)| at < *first) {
            earliest = Some((at, snapshot.observations));
        }
    }
    let (_, observations) = effective
        .or(earliest)
        .ok_or_else(|| FixtureError("market path has no snapshots".to_owned()))?;
    normalize(observations)
}

#[cfg(test)]
mod tests {
    use super::{InstrumentKind, MarketLeg, MarketStatus, load, load_at};
    use chrono::{TimeZone, Utc};
    use funding_arb_domain::market::Venue;
    use rust_decimal::Decimal;

    fn bybit_btc_perp(legs: &[MarketLeg]) -> &MarketLeg {
        legs.iter()
            .find(|leg| {
                leg.observation.instrument.venue == Venue::Bybit
                    && leg.observation.instrument.underlying == "BTC"
                    && leg.kind == InstrumentKind::Perp
            })
            .expect("bybit btc perp present")
    }

    #[test]
    fn baseline_snapshot_matches_markets_json() {
        let t0 = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let base = load().expect("load");
        let at_t0 = load_at(t0).expect("load_at");
        assert_eq!(base.len(), at_t0.len());
        for (a, b) in base.iter().zip(&at_t0) {
            assert_eq!(
                a.observation.instrument.symbol,
                b.observation.instrument.symbol
            );
            assert_eq!(a.observation.mark_price, b.observation.mark_price);
            assert_eq!(a.observation.funding_rate, b.observation.funding_rate);
        }
    }

    #[test]
    fn path_evolves_marks_then_funding() {
        let t1 = Utc.with_ymd_and_hms(2026, 7, 19, 4, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 7, 19, 8, 0, 0).unwrap();
        let t3 = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        // t1 and t2: BYBIT BTC basis stays wide (persistence → re-hedge).
        assert_eq!(
            bybit_btc_perp(&load_at(t1).unwrap()).observation.mark_price,
            Decimal::from(32_000),
        );
        assert_eq!(
            bybit_btc_perp(&load_at(t2).unwrap()).observation.mark_price,
            Decimal::from(32_000),
        );
        // t3: BYBIT BTC funding flips negative (→ auto-exit).
        assert!(
            bybit_btc_perp(&load_at(t3).unwrap())
                .observation
                .funding_rate
                .is_sign_negative()
        );
    }

    #[test]
    fn before_first_snapshot_falls_back_to_earliest() {
        let before = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(
            bybit_btc_perp(&load_at(before).unwrap())
                .observation
                .mark_price,
            Decimal::from(30_045),
        );
    }

    #[test]
    fn t4_degrades_bybit_without_changing_earlier_ticks() {
        let t4 = Utc.with_ymd_and_hms(2026, 7, 19, 16, 0, 0).unwrap();
        assert_eq!(
            bybit_btc_perp(&load_at(t4).unwrap()).status,
            MarketStatus::Degraded
        );
    }
}
