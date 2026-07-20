//! Deterministic local order book with fail-closed sequence handling.

use std::collections::BTreeMap;

use rust_decimal::Decimal;

use crate::{InstrumentId, MarketEvent, PriceLevel};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BookStatus {
    AwaitingSnapshot,
    Current,
    Stale { reason: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BookApply {
    Applied,
    IgnoredDuplicate,
    ResetRequired,
    Unrelated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookView {
    pub sequence: u64,
    pub best_bid: Option<PriceLevel>,
    pub best_ask: Option<PriceLevel>,
    pub crossed: bool,
}

/// Venue-neutral book state. A gap invalidates all prices until a new snapshot.
pub struct BookState {
    instrument: InstrumentId,
    status: BookStatus,
    sequence: Option<u64>,
    bids: BTreeMap<Decimal, Decimal>,
    asks: BTreeMap<Decimal, Decimal>,
    checksum: Option<String>,
}

impl BookState {
    #[must_use]
    pub fn new(instrument: InstrumentId) -> Self {
        Self {
            instrument,
            status: BookStatus::AwaitingSnapshot,
            sequence: None,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            checksum: None,
        }
    }

    #[must_use]
    pub const fn status(&self) -> &BookStatus {
        &self.status
    }

    #[must_use]
    pub fn checksum(&self) -> Option<&str> {
        self.checksum.as_deref()
    }

    pub fn apply(&mut self, event: &MarketEvent) -> BookApply {
        match event {
            MarketEvent::BookSnapshot {
                instrument,
                sequence,
                bids,
                asks,
                checksum,
                ..
            } if instrument == &self.instrument => {
                self.bids.clear();
                self.asks.clear();
                apply_levels(&mut self.bids, bids);
                apply_levels(&mut self.asks, asks);
                self.sequence = Some(*sequence);
                self.checksum.clone_from(checksum);
                self.status = BookStatus::Current;
                BookApply::Applied
            }
            MarketEvent::BookDelta {
                instrument,
                first_sequence,
                last_sequence,
                bids,
                asks,
                checksum,
                ..
            } if instrument == &self.instrument => {
                let Some(current) = self.sequence else {
                    self.status = BookStatus::Stale {
                        reason: "DELTA_BEFORE_SNAPSHOT".into(),
                    };
                    return BookApply::ResetRequired;
                };
                if !matches!(self.status, BookStatus::Current) {
                    return BookApply::ResetRequired;
                }
                if *last_sequence <= current {
                    return BookApply::IgnoredDuplicate;
                }
                let expected = current.saturating_add(1);
                if *first_sequence > expected || *last_sequence < expected {
                    self.status = BookStatus::Stale {
                        reason: format!(
                            "SEQUENCE_GAP_EXPECTED_{expected}_GOT_{first_sequence}_{last_sequence}"
                        ),
                    };
                    self.bids.clear();
                    self.asks.clear();
                    self.checksum = None;
                    return BookApply::ResetRequired;
                }
                apply_levels(&mut self.bids, bids);
                apply_levels(&mut self.asks, asks);
                self.sequence = Some(*last_sequence);
                self.checksum.clone_from(checksum);
                BookApply::Applied
            }
            _ => BookApply::Unrelated,
        }
    }

    #[must_use]
    pub fn view(&self) -> Option<BookView> {
        if !matches!(self.status, BookStatus::Current) {
            return None;
        }
        let best_bid = self
            .bids
            .last_key_value()
            .map(|(price, quantity)| PriceLevel {
                price: *price,
                quantity: *quantity,
            });
        let best_ask = self
            .asks
            .first_key_value()
            .map(|(price, quantity)| PriceLevel {
                price: *price,
                quantity: *quantity,
            });
        let crossed = best_bid
            .as_ref()
            .zip(best_ask.as_ref())
            .is_some_and(|(bid, ask)| bid.price >= ask.price);
        Some(BookView {
            sequence: self.sequence.unwrap_or_default(),
            best_bid,
            best_ask,
            crossed,
        })
    }

    /// Quantity-weighted executable price for a base quantity.
    #[must_use]
    pub fn executable_price(&self, buy: bool, quantity: Decimal) -> Option<Decimal> {
        if quantity <= Decimal::ZERO || !matches!(self.status, BookStatus::Current) {
            return None;
        }
        let levels: Box<dyn Iterator<Item = (&Decimal, &Decimal)> + '_> = if buy {
            Box::new(self.asks.iter())
        } else {
            Box::new(self.bids.iter().rev())
        };
        let mut remaining = quantity;
        let mut notional = Decimal::ZERO;
        for (price, available) in levels {
            let used = remaining.min(*available);
            notional += used * *price;
            remaining -= used;
            if remaining <= Decimal::ZERO {
                return Some(notional / quantity);
            }
        }
        None
    }
}

fn apply_levels(book: &mut BTreeMap<Decimal, Decimal>, levels: &[PriceLevel]) {
    for level in levels {
        if level.quantity <= Decimal::ZERO {
            book.remove(&level.price);
        } else {
            book.insert(level.price, level.quantity);
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use funding_arb_domain::market::Venue;
    use rust_decimal::Decimal;

    use super::*;
    use crate::ProductType;

    fn instrument() -> InstrumentId {
        InstrumentId {
            venue: Venue::Bybit,
            symbol: "BTCUSDT".into(),
            product: ProductType::LinearPerpetual,
        }
    }

    fn level(price: i64, quantity: i64) -> PriceLevel {
        PriceLevel {
            price: Decimal::new(price, 0),
            quantity: Decimal::new(quantity, 0),
        }
    }

    #[test]
    fn gap_clears_prices_until_a_new_snapshot() {
        let instrument = instrument();
        let now = Utc::now();
        let mut book = BookState::new(instrument.clone());
        assert_eq!(
            book.apply(&MarketEvent::BookSnapshot {
                instrument: instrument.clone(),
                sequence: 10,
                bids: vec![level(99, 2)],
                asks: vec![level(101, 2)],
                venue_time: now,
                received_at: now,
                checksum: None,
            }),
            BookApply::Applied
        );
        assert_eq!(
            book.apply(&MarketEvent::BookDelta {
                instrument,
                first_sequence: 12,
                last_sequence: 12,
                bids: vec![],
                asks: vec![],
                venue_time: now,
                received_at: now,
                checksum: None,
            }),
            BookApply::ResetRequired
        );
        assert!(book.view().is_none());
    }

    #[test]
    fn overlapping_delta_and_exact_vwap_are_deterministic() {
        let instrument = instrument();
        let now = Utc::now();
        let mut book = BookState::new(instrument.clone());
        book.apply(&MarketEvent::BookSnapshot {
            instrument: instrument.clone(),
            sequence: 10,
            bids: vec![level(99, 5)],
            asks: vec![level(100, 1), level(102, 2)],
            venue_time: now,
            received_at: now,
            checksum: None,
        });
        assert_eq!(
            book.apply(&MarketEvent::BookDelta {
                instrument,
                first_sequence: 10,
                last_sequence: 11,
                bids: vec![],
                asks: vec![],
                venue_time: now,
                received_at: now,
                checksum: None,
            }),
            BookApply::Applied
        );
        assert_eq!(
            book.executable_price(true, Decimal::new(2, 0)),
            Some(Decimal::new(101, 0))
        );
    }
}
