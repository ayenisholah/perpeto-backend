//! Deterministic funding-arbitrage scanner service (M2 slice 1).
//!
//! Loads fixture markets, forecasts funding, ranks two-leg opportunities
//! (`funding_arb_domain`), and persists/reads scan snapshots. Paper-only and
//! network-free; venue connectors arrive in M3.

pub mod dto;
pub mod fixtures;
pub mod repo;
pub mod service;

pub use dto::{LegDto, OpportunityDto, OpportunityScanDto, RiskLimitsDto};
pub use repo::ScannerRepo;
pub use service::{RankedOpportunity, ScanResult, Scanner};
