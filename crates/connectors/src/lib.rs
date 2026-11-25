//! Capability-driven CEX connector boundary for M3.

mod binance;
mod book;
mod bybit;
mod credential;
pub mod fixtures;
pub mod http;
mod model;
mod okx;
mod traits;
mod ws;

pub use binance::BinanceConnector;
pub use book::{BookApply, BookState, BookStatus, BookView};
pub use bybit::BybitConnector;
pub use credential::ConnectorCredentials;
pub use model::*;
pub use okx::OkxConnector;
pub use traits::{MarketEventStream, PrivateEventStream, VenueConnector};
