//! Versioned paper strategy application service.

mod dto;
mod repo;
mod service;

pub use dto::{AllocationDto, AutoCompoundDto, StrategyDto, StrategyVersionDto};
pub use repo::StrategiesRepo;
pub use service::{OpenIntent, PositionSummary, StrategyService};
