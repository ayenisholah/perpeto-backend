//! Core types that are safe to share across Signex backend services.
//!
//! This crate is intentionally small during M0. Financial value types and
//! state machines are introduced only with their specification-backed tests.

use std::{fmt, str::FromStr};

pub mod auth;

/// Deployment trading mode. The initial scaffold supports paper mode only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TradingMode {
    Paper,
    Live,
}

impl TradingMode {
    /// Returns true only for the development-safe paper mode.
    #[must_use]
    pub const fn is_paper(self) -> bool {
        matches!(self, Self::Paper)
    }
}

impl FromStr for TradingMode {
    type Err = ParseTradingModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_uppercase().as_str() {
            "PAPER" => Ok(Self::Paper),
            "LIVE" => Ok(Self::Live),
            _ => Err(ParseTradingModeError),
        }
    }
}

/// Returned when a configured trading mode is not recognized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseTradingModeError;

impl fmt::Display for ParseTradingModeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("trading mode must be PAPER or LIVE")
    }
}

impl std::error::Error for ParseTradingModeError {}

#[cfg(test)]
mod tests {
    use super::TradingMode;
    use std::str::FromStr;

    #[test]
    fn paper_mode_is_recognized_case_insensitively() {
        assert_eq!(TradingMode::from_str("paper"), Ok(TradingMode::Paper));
        assert!(TradingMode::Paper.is_paper());
    }

    #[test]
    fn unknown_mode_fails_closed() {
        assert!(TradingMode::from_str("preview").is_err());
    }
}
