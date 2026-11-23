//! Deterministic funding-arbitrage scanner service (M2 slice 1).
//!
//! Loads fixture markets, forecasts funding, ranks two-leg opportunities
//! (`funding_arb_domain`), and persists/reads scan snapshots. Paper-only and
//! network-free; venue connectors arrive in M3.

pub mod dto;
pub mod fixtures;
pub mod market;
pub mod repo;
pub mod service;

pub use dto::{LegDto, OpportunityDto, OpportunityScanDto, RiskLimitsDto};
pub use market::{
    BookSide, FeedStatus, LegBook, MarketSnapshot, MarketSnapshotDto, SnapshotDecodeError,
    VenueFeedStatus,
};
pub use repo::{ScannerRepo, StoredSnapshot};
pub use service::{RankedOpportunity, ScanResult, Scanner};
