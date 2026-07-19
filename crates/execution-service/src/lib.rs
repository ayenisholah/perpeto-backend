//! Paper execution service (M2 slice 3).
//!
//! Opens a **paper** arbitrage position from a ranked opportunity: it re-runs
//! the deterministic scanner to locate the route, sizes and simulates a paired
//! taker fill via `funding_arb_domain::position`, and persists the position for
//! the API to serve. Paper-only and network-free; venue connectors arrive in M3
//! and funding capture / close arrive in the next slice.

pub mod dto;
pub mod repo;
pub mod service;

pub use dto::{OpenPositionRequest, PositionDto, PositionLegDto, PositionListDto};
pub use repo::PositionsRepo;
pub use service::{ExecutionService, OpenError};
