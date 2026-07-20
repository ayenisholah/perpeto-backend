//! Capability-driven CEX connector boundary for M3.

mod model;
mod traits;

pub use model::*;
pub use traits::{MarketEventStream, PrivateEventStream, VenueConnector};
