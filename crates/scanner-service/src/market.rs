//! The market snapshot behind one scan (DEC-0016).
//!
//! A [`MarketSnapshot`] holds the legs the scanner ranked, the order book
//! behind each leg (for paper fills) and per-venue feed health. The engine
//! persists it beside the scan so the API opens and the engine supervises paper
//! positions against exactly the data that was ranked, rather than reloading
//! fixtures or re-reading venues.
//!
//! [`MarketSnapshotDto`] is the persisted form. Decimals serialize as strings
//! (`rust_decimal` `serde-with-str`), so a round trip is exact.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use funding_arb_domain::market::{FundingObservation, InstrumentRef, Venue};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::fixtures::{InstrumentKind, MarketLeg, MarketStatus};

/// One side of a stored order book, best level first.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BookSide {
    /// Price levels as (price, quantity in contracts), best first.
    pub levels: Vec<(Decimal, Decimal)>,
}

/// The order book captured for one leg, used to simulate paper fills.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegBook {
    /// The venue of the leg.
    pub venue: Venue,
    /// The venue symbol of the leg.
    pub symbol: String,
    /// Contract multiplier that converts quantity to base units.
    pub contract_multiplier: Decimal,
    /// Bids, best (highest) first.
    pub bids: BookSide,
    /// Asks, best (lowest) first.
    pub asks: BookSide,
}

/// Health of one venue's public feed in a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedStatus {
    /// Every requested call succeeded and the venue reported healthy.
    Healthy,
    /// Data arrived but the venue reported degraded health.
    Degraded,
    /// The venue could not be read; it contributed no legs.
    Unavailable,
}

impl FeedStatus {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "HEALTHY",
            Self::Degraded => "DEGRADED",
            Self::Unavailable => "UNAVAILABLE",
        }
    }
}

/// Per-venue feed report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VenueFeedStatus {
    /// The venue.
    pub venue: Venue,
    /// Feed health.
    pub status: FeedStatus,
    /// The newest observation time from this venue, if any.
    pub observed_at: Option<DateTime<Utc>>,
    /// Why the venue is unavailable, when it is.
    pub error: Option<String>,
}

/// One scan's worth of market data.
#[derive(Clone, Debug)]
pub struct MarketSnapshot {
    /// When the snapshot was taken.
    pub taken_at: DateTime<Utc>,
    /// Normalized legs for the scanner.
    pub legs: Vec<MarketLeg>,
    /// The book behind each leg, for paper fills. Empty for fixture snapshots.
    pub books: Vec<LegBook>,
    /// Per-venue feed health. Empty for fixture snapshots.
    pub venues: Vec<VenueFeedStatus>,
}

impl MarketSnapshot {
    /// A fixture snapshot: legs only, with no books or feed reports.
    #[must_use]
    pub fn from_legs(taken_at: DateTime<Utc>, legs: Vec<MarketLeg>) -> Self {
        Self {
            taken_at,
            legs,
            books: Vec::new(),
            venues: Vec::new(),
        }
    }

    /// The stored book for a leg, if one was captured.
    #[must_use]
    pub fn book(&self, venue: Venue, symbol: &str) -> Option<&LegBook> {
        self.books
            .iter()
            .find(|book| book.venue == venue && book.symbol == symbol)
    }
}

/// Raised when a persisted snapshot cannot be decoded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotDecodeError(String);

impl fmt::Display for SnapshotDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "market snapshot decode error: {}", self.0)
    }
}

impl std::error::Error for SnapshotDecodeError {}

fn venue(value: &str) -> Result<Venue, SnapshotDecodeError> {
    Venue::from_str(value).map_err(|_| SnapshotDecodeError(format!("unknown venue {value:?}")))
}

/// Persisted form of a [`MarketLeg`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MarketLegDto {
    /// `SPOT` or `PERP`.
    pub kind: String,
    /// `HEALTHY` or `DEGRADED`.
    pub status: String,
    /// Venue wire spelling.
    pub venue: String,
    /// Venue symbol.
    pub symbol: String,
    /// Canonical underlying.
    pub underlying: String,
    /// Funding rate for one interval.
    pub funding_rate: Decimal,
    /// Funding interval in hours.
    pub interval_hours: u32,
    /// Mark price.
    pub mark_price: Decimal,
    /// Index price.
    pub index_price: Decimal,
    /// Executable notional on the thinner side of the book.
    pub available_notional: Decimal,
    /// When the leg was observed.
    pub observed_at: DateTime<Utc>,
    /// When funding next settles.
    pub next_funding_at: DateTime<Utc>,
}

/// Persisted form of a [`LegBook`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LegBookDto {
    /// Venue wire spelling.
    pub venue: String,
    /// Venue symbol.
    pub symbol: String,
    /// Contract multiplier.
    pub contract_multiplier: Decimal,
    /// Bids, best first.
    pub bids: BookSide,
    /// Asks, best first.
    pub asks: BookSide,
}

/// Persisted form of a [`VenueFeedStatus`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VenueFeedStatusDto {
    /// Venue wire spelling.
    pub venue: String,
    /// `HEALTHY`, `DEGRADED` or `UNAVAILABLE`.
    pub status: String,
    /// Newest observation from this venue.
    pub observed_at: Option<DateTime<Utc>>,
    /// Failure reason, when unavailable.
    pub error: Option<String>,
}

/// Persisted form of a [`MarketSnapshot`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MarketSnapshotDto {
    /// When the snapshot was taken.
    pub taken_at: DateTime<Utc>,
    /// The legs.
    pub legs: Vec<MarketLegDto>,
    /// The books.
    pub books: Vec<LegBookDto>,
    /// The venue feed reports.
    pub venues: Vec<VenueFeedStatusDto>,
}

impl MarketSnapshotDto {
    /// Encode a snapshot for persistence.
    #[must_use]
    pub fn from_domain(snapshot: &MarketSnapshot) -> Self {
        Self {
            taken_at: snapshot.taken_at,
            legs: snapshot
                .legs
                .iter()
                .map(|leg| MarketLegDto {
                    kind: match leg.kind {
                        InstrumentKind::Spot => "SPOT",
                        InstrumentKind::Perp => "PERP",
                    }
                    .to_owned(),
                    status: match leg.status {
                        MarketStatus::Healthy => "HEALTHY",
                        MarketStatus::Degraded => "DEGRADED",
                    }
                    .to_owned(),
                    venue: leg.observation.instrument.venue.as_str().to_owned(),
                    symbol: leg.observation.instrument.symbol.clone(),
                    underlying: leg.observation.instrument.underlying.clone(),
                    funding_rate: leg.observation.funding_rate,
                    interval_hours: leg.observation.interval_hours,
                    mark_price: leg.observation.mark_price,
                    index_price: leg.observation.index_price,
                    available_notional: leg.observation.available_notional,
                    observed_at: leg.observation.observed_at,
                    next_funding_at: leg.observation.next_funding_at,
                })
                .collect(),
            books: snapshot
                .books
                .iter()
                .map(|book| LegBookDto {
                    venue: book.venue.as_str().to_owned(),
                    symbol: book.symbol.clone(),
                    contract_multiplier: book.contract_multiplier,
                    bids: book.bids.clone(),
                    asks: book.asks.clone(),
                })
                .collect(),
            venues: snapshot
                .venues
                .iter()
                .map(|feed| VenueFeedStatusDto {
                    venue: feed.venue.as_str().to_owned(),
                    status: feed.status.as_str().to_owned(),
                    observed_at: feed.observed_at,
                    error: feed.error.clone(),
                })
                .collect(),
        }
    }

    /// Decode a persisted snapshot.
    ///
    /// # Errors
    /// Returns [`SnapshotDecodeError`] for an unknown venue, kind or status.
    pub fn to_domain(&self) -> Result<MarketSnapshot, SnapshotDecodeError> {
        let legs = self
            .legs
            .iter()
            .map(|leg| {
                Ok(MarketLeg {
                    kind: match leg.kind.as_str() {
                        "SPOT" => InstrumentKind::Spot,
                        "PERP" => InstrumentKind::Perp,
                        other => {
                            return Err(SnapshotDecodeError(format!("unknown leg kind {other:?}")));
                        }
                    },
                    status: match leg.status.as_str() {
                        "HEALTHY" => MarketStatus::Healthy,
                        "DEGRADED" => MarketStatus::Degraded,
                        other => {
                            return Err(SnapshotDecodeError(format!(
                                "unknown leg status {other:?}"
                            )));
                        }
                    },
                    observation: FundingObservation {
                        instrument: InstrumentRef {
                            venue: venue(&leg.venue)?,
                            symbol: leg.symbol.clone(),
                            underlying: leg.underlying.clone(),
                        },
                        funding_rate: leg.funding_rate,
                        interval_hours: leg.interval_hours,
                        mark_price: leg.mark_price,
                        index_price: leg.index_price,
                        available_notional: leg.available_notional,
                        observed_at: leg.observed_at,
                        next_funding_at: leg.next_funding_at,
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let books = self
            .books
            .iter()
            .map(|book| {
                Ok(LegBook {
                    venue: venue(&book.venue)?,
                    symbol: book.symbol.clone(),
                    contract_multiplier: book.contract_multiplier,
                    bids: book.bids.clone(),
                    asks: book.asks.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let venues = self
            .venues
            .iter()
            .map(|feed| {
                Ok(VenueFeedStatus {
                    venue: venue(&feed.venue)?,
                    status: match feed.status.as_str() {
                        "HEALTHY" => FeedStatus::Healthy,
                        "DEGRADED" => FeedStatus::Degraded,
                        "UNAVAILABLE" => FeedStatus::Unavailable,
                        other => {
                            return Err(SnapshotDecodeError(format!(
                                "unknown feed status {other:?}"
                            )));
                        }
                    },
                    observed_at: feed.observed_at,
                    error: feed.error.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MarketSnapshot {
            taken_at: self.taken_at,
            legs,
            books,
            venues,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 22, hour, 0, 0)
            .single()
            .unwrap_or_default()
    }

    fn dec(value: &str) -> Decimal {
        value.parse().unwrap_or_default()
    }

    fn snapshot() -> MarketSnapshot {
        MarketSnapshot {
            taken_at: at(12),
            legs: vec![MarketLeg {
                kind: InstrumentKind::Perp,
                status: MarketStatus::Degraded,
                observation: FundingObservation {
                    instrument: InstrumentRef {
                        venue: Venue::Okx,
                        symbol: "BTC-USDT-SWAP".into(),
                        underlying: "BTC".into(),
                    },
                    // Deliberately beyond f64 precision.
                    funding_rate: dec("0.000123456789012345678"),
                    interval_hours: 8,
                    mark_price: dec("97123.45678901234"),
                    index_price: dec("97120.1"),
                    available_notional: dec("1234567.890123456789"),
                    observed_at: at(12),
                    next_funding_at: at(16),
                },
            }],
            books: vec![LegBook {
                venue: Venue::Okx,
                symbol: "BTC-USDT-SWAP".into(),
                contract_multiplier: dec("0.01"),
                bids: BookSide {
                    levels: vec![(dec("97120.1"), dec("3.5"))],
                },
                asks: BookSide {
                    levels: vec![(dec("97120.2"), dec("0.000000001"))],
                },
            }],
            venues: vec![
                VenueFeedStatus {
                    venue: Venue::Okx,
                    status: FeedStatus::Degraded,
                    observed_at: Some(at(12)),
                    error: None,
                },
                VenueFeedStatus {
                    venue: Venue::Bybit,
                    status: FeedStatus::Unavailable,
                    observed_at: None,
                    error: Some("health timed out".into()),
                },
            ],
        }
    }

    #[test]
    fn a_json_round_trip_is_exact() {
        let dto = MarketSnapshotDto::from_domain(&snapshot());
        let json = serde_json::to_string(&dto).unwrap_or_default();
        // Decimals are strings on the wire, never floats.
        assert!(json.contains("\"0.000123456789012345678\""));
        let decoded: Result<MarketSnapshotDto, _> = serde_json::from_str(&json);
        assert_eq!(decoded.as_ref().ok(), Some(&dto));
        let restored = dto.to_domain();
        assert!(restored.is_ok());
        let restored = restored.unwrap_or_else(|_| MarketSnapshot::from_legs(at(0), Vec::new()));
        assert_eq!(MarketSnapshotDto::from_domain(&restored), dto);
        assert_eq!(
            restored.legs[0].observation.funding_rate,
            dec("0.000123456789012345678")
        );
        assert_eq!(
            restored.venues[1].error.as_deref(),
            Some("health timed out")
        );
    }

    #[test]
    fn unknown_enumerations_fail_to_decode() {
        let mut dto = MarketSnapshotDto::from_domain(&snapshot());
        dto.legs[0].kind = "FUTURE".into();
        assert!(dto.to_domain().is_err());

        let mut dto = MarketSnapshotDto::from_domain(&snapshot());
        dto.venues[0].status = "MAYBE".into();
        assert!(dto.to_domain().is_err());

        let mut dto = MarketSnapshotDto::from_domain(&snapshot());
        dto.books[0].venue = "NYSE".into();
        assert!(dto.to_domain().is_err());

        let mut dto = MarketSnapshotDto::from_domain(&snapshot());
        dto.legs[0].status = "UNKNOWN".into();
        assert!(dto.to_domain().is_err());
    }

    #[test]
    fn books_are_found_by_venue_and_symbol() {
        let snapshot = snapshot();
        assert!(snapshot.book(Venue::Okx, "BTC-USDT-SWAP").is_some());
        assert!(snapshot.book(Venue::Bybit, "BTC-USDT-SWAP").is_none());
        assert!(snapshot.book(Venue::Okx, "ETH-USDT-SWAP").is_none());
    }

    #[test]
    fn fixture_snapshots_carry_legs_only() {
        let snapshot = MarketSnapshot::from_legs(at(1), Vec::new());
        assert!(snapshot.books.is_empty() && snapshot.venues.is_empty());
        assert_eq!(FeedStatus::Unavailable.as_str(), "UNAVAILABLE");
    }
}
