//! Capability-driven CEX connector boundary for M3.

pub mod fixtures;
mod model;
mod traits;

pub use model::*;
pub use traits::{MarketEventStream, PrivateEventStream, VenueConnector};
