//! Transparent, deterministic funding-rate forecasting (no ML).
//!
//! The v1 predictor is intentionally explainable: it blends the annualized
//! current funding rate with the annualized perpetual premium, then derives a
//! symmetric p10/p90 band and a direction probability. Given the same
//! observation and config it always returns the same forecast, so scans are
//! reproducible and unit-testable.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::market::{FundingObservation, InstrumentRef, Venue};

/// The forecasting model identifier recorded on every scan.
pub const MODEL_VERSION: &str = "funding-forecast-v1";

/// A per-leg funding forecast derived from a single observation.
#[derive(Clone, Debug)]
pub struct FundingForecast {
    /// The venue of the forecast instrument.
    pub venue: Venue,
    /// The instrument the forecast is for.
    pub instrument: InstrumentRef,
    /// The annualized current funding rate.
    pub current_estimate: Decimal,
    /// The annualized predicted settlement rate.
    pub predicted_settlement_rate: Decimal,
    /// The lower (10th percentile) annualized-rate bound.
    pub p10_rate: Decimal,
    /// The upper (90th percentile) annualized-rate bound.
    pub p90_rate: Decimal,
    /// Probability the settlement rate keeps its predicted sign, in `[0.5, 1]`.
    pub direction_probability: Decimal,
    /// The perpetual premium `(mark - index) / index` at observation time.
    pub premium: Decimal,
    /// When funding next settles.
    pub next_funding_at: DateTime<Utc>,
    /// The forecasting model version.
    pub model_version: &'static str,
    /// The timestamp of the observation the forecast was built from.
    pub input_freshness: DateTime<Utc>,
}

/// Tunable, transparent parameters for the forecaster.
#[derive(Clone, Copy, Debug)]
pub struct ForecastConfig {
    /// The p10/p90 half-band as a fraction of `|predicted|`.
    pub residual_band_ratio: Decimal,
    /// How strongly the annualized premium pulls the prediction off the current rate.
    pub premium_weight: Decimal,
    /// A floor for the half-band, in annualized-rate units.
    pub min_band: Decimal,
}

impl Default for ForecastConfig {
    fn default() -> Self {
        Self {
            residual_band_ratio: Decimal::new(25, 2), // 0.25
            premium_weight: Decimal::new(50, 2),      // 0.50
            min_band: Decimal::new(1, 2),             // 0.01 (1% APR)
        }
    }
}

fn abs(value: Decimal) -> Decimal {
    if value.is_sign_negative() {
        -value
    } else {
        value
    }
}

/// Produce a deterministic forecast for one observation.
#[must_use]
pub fn forecast(observation: &FundingObservation, config: &ForecastConfig) -> FundingForecast {
    let current = observation.annualized_rate();
    let premium_apr = observation.premium() * observation.intervals_per_year();
    let predicted = current + (premium_apr - current) * config.premium_weight;

    let half_band = {
        let scaled = abs(predicted) * config.residual_band_ratio;
        if scaled > config.min_band {
            scaled
        } else {
            config.min_band
        }
    };

    // P(rate keeps its sign): distance of the prediction from zero relative to
    // the band, mapped into [0.5, 1].
    let denominator = abs(predicted) + half_band;
    let direction_probability = if denominator.is_zero() {
        Decimal::new(5, 1)
    } else {
        Decimal::new(5, 1) + Decimal::new(5, 1) * (abs(predicted) / denominator)
    };

    FundingForecast {
        venue: observation.instrument.venue,
        instrument: observation.instrument.clone(),
        current_estimate: current,
        predicted_settlement_rate: predicted,
        p10_rate: predicted - half_band,
        p90_rate: predicted + half_band,
        direction_probability,
        premium: observation.premium(),
        next_funding_at: observation.next_funding_at,
        model_version: MODEL_VERSION,
        input_freshness: observation.observed_at,
    }
}
