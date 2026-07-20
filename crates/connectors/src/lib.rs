//! Capability-driven CEX connector boundary for M3.

mod book;
pub mod fixtures;
mod model;
mod traits;

pub use book::{BookApply, BookState, BookStatus, BookView};
pub use model::*;
pub use traits::{MarketEventStream, PrivateEventStream, VenueConnector};
