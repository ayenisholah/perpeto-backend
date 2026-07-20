//! Paper portfolio, health, and alert read models.

use crate::{BreakersRepo, PositionDto, PositionsRepo};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;
use uuid::Uuid;

fn number(value: Decimal) -> f64 {
    value.to_f64().unwrap_or_default()
}
fn decimal(value: f64) -> Decimal {
    Decimal::from_f64_retain(value).unwrap_or_default()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AllocationDto {
    pub key: String,
    pub deployed_capital: f64,
    pub fraction: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PnlDto {
    pub realized_funding: f64,
    pub realized_trading: f64,
    pub unrealized_leg: f64,
    pub fees: f64,
    pub net: f64,
    pub return_on_deployed_capital: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortfolioDto {
    pub equity: f64,
    pub deployable_equity: f64,
    pub deployed_capital: f64,
    pub by_venue: Vec<AllocationDto>,
    pub by_underlying: Vec<AllocationDto>,
    pub pnl: PnlDto,
    pub as_of: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VenueHealthDto {
    pub venue: String,
    pub status: String,
    pub connected: bool,
    pub market_data_age_seconds: i64,
    pub clock_drift_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceHeartbeatDto {
    pub service: String,
    pub status: String,
    pub observed_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HealthDto {
    pub venues: Vec<VenueHealthDto>,
    pub services: Vec<ServiceHeartbeatDto>,
    pub as_of: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertDto {
    pub id: String,
    pub dedup_key: String,
    pub severity: String,
    pub title: String,
    pub message: String,
    pub created_at: String,
    pub acknowledged_at: Option<String>,
    pub acknowledged_by: Option<String>,
}

#[derive(Clone)]
pub struct MonitoringRepo {
    pool: PgPool,
    positions: PositionsRepo,
    breakers: BreakersRepo,
}

impl MonitoringRepo {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            positions: PositionsRepo::new(pool.clone()),
            breakers: BreakersRepo::new(pool.clone()),
            pool,
        }
    }
    pub async fn portfolio(&self, now: DateTime<Utc>) -> Result<PortfolioDto, sqlx::Error> {
        Ok(aggregate(&self.positions.list().await?, now))
    }
    pub async fn health(&self, now: DateTime<Utc>) -> Result<HealthDto, sqlx::Error> {
        let controls = self.breakers.controls().await?;
        let rows = sqlx::query("SELECT DISTINCT ON (service) service, status, observed_at FROM service_heartbeats ORDER BY service, observed_at DESC").fetch_all(&self.pool).await?;
        let services = rows
            .iter()
            .map(|row| ServiceHeartbeatDto {
                service: row.get("service"),
                status: row.get("status"),
                observed_at: row.get::<DateTime<Utc>, _>("observed_at").to_rfc3339(),
            })
            .collect();
        let venues = ["BINANCE", "BYBIT", "OKX"]
            .into_iter()
            .map(|venue| {
                let degraded = controls
                    .breakers
                    .iter()
                    .any(|breaker| breaker.scope_key.as_deref() == Some(venue));
                VenueHealthDto {
                    venue: venue.into(),
                    status: if degraded {
                        "DEGRADED".into()
                    } else {
                        "HEALTHY".into()
                    },
                    connected: !degraded,
                    market_data_age_seconds: 0,
                    clock_drift_ms: 0,
                }
            })
            .collect();
        Ok(HealthDto {
            venues,
            services,
            as_of: now.to_rfc3339(),
        })
    }
    pub async fn alerts(&self) -> Result<Vec<AlertDto>, sqlx::Error> {
        sqlx::query("SELECT payload FROM alerts ORDER BY created_at DESC, id DESC")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|row| {
                serde_json::from_str(&row.get::<String, _>("payload"))
                    .map_err(|error| sqlx::Error::Protocol(error.to_string()))
            })
            .collect()
    }
    pub async fn acknowledge(
        &self,
        id: &str,
        user: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<AlertDto>, sqlx::Error> {
        let id = Uuid::parse_str(id).map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let Some(row) = sqlx::query("SELECT payload FROM alerts WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };
        let mut alert: AlertDto = serde_json::from_str(&row.get::<String, _>("payload"))
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        if alert.acknowledged_at.is_none() {
            alert.acknowledged_at = Some(now.to_rfc3339());
            alert.acknowledged_by = Some(user.into());
        }
        let payload = serde_json::to_string(&alert)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let user_id =
            Uuid::parse_str(user).map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        sqlx::query("UPDATE alerts SET acknowledged_at = $2, acknowledged_by = $3, payload = $4 WHERE id = $1").bind(id).bind(now).bind(user_id).bind(payload).execute(&self.pool).await?;
        Ok(Some(alert))
    }
}

fn aggregate(positions: &[PositionDto], now: DateTime<Utc>) -> PortfolioDto {
    let reference = Decimal::from(100_000);
    let mut deployed = Decimal::ZERO;
    let mut venues = BTreeMap::<String, Decimal>::new();
    let mut assets = BTreeMap::<String, Decimal>::new();
    let mut funding = Decimal::ZERO;
    let mut trading = Decimal::ZERO;
    let mut unrealized = Decimal::ZERO;
    let mut fees = Decimal::ZERO;
    for position in positions {
        let capital = decimal(position.reserved_capital);
        if position.state != "CLOSED" {
            deployed += capital;
        }
        *assets.entry(position.underlying.clone()).or_default() += capital;
        *venues.entry(position.long_leg.venue.clone()).or_default() += capital / Decimal::from(2);
        *venues.entry(position.short_leg.venue.clone()).or_default() += capital / Decimal::from(2);
        funding += decimal(position.funding_captured);
        let position_fees = decimal(position.long_leg.fees + position.short_leg.fees);
        fees += position_fees;
        if position.state == "CLOSED" {
            trading +=
                decimal(position.realized_pnl) - decimal(position.funding_captured) + position_fees;
        } else {
            unrealized += decimal(position.long_leg.filled_qty)
                * (decimal(position.long_leg.mark_price)
                    - decimal(position.long_leg.avg_fill_price))
                + decimal(position.short_leg.filled_qty)
                    * (decimal(position.short_leg.avg_fill_price)
                        - decimal(position.short_leg.mark_price));
        }
    }
    let net = funding + trading + unrealized - fees;
    let allocation = |values: BTreeMap<String, Decimal>| {
        values
            .into_iter()
            .map(|(key, value)| AllocationDto {
                key,
                deployed_capital: number(value),
                fraction: number(if deployed.is_zero() {
                    Decimal::ZERO
                } else {
                    value / deployed
                }),
            })
            .collect()
    };
    PortfolioDto {
        equity: number(reference + net),
        deployable_equity: number((reference - deployed).max(Decimal::ZERO)),
        deployed_capital: number(deployed),
        by_venue: allocation(venues),
        by_underlying: allocation(assets),
        pnl: PnlDto {
            realized_funding: number(funding),
            realized_trading: number(trading),
            unrealized_leg: number(unrealized),
            fees: number(fees),
            net: number(net),
            return_on_deployed_capital: number(if deployed.is_zero() {
                Decimal::ZERO
            } else {
                net / deployed
            }),
        },
        as_of: now.to_rfc3339(),
    }
}
