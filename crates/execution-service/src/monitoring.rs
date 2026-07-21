//! Paper portfolio, health, and alert read models.

use crate::{BreakersRepo, PositionDto, PositionsRepo};
use chrono::{DateTime, Utc};
use funding_arb_tenant_context::{TenantContext, TenantTransaction};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AllocationDto {
    pub key: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub deployed_capital: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub fraction: Decimal,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PnlDto {
    #[serde(with = "rust_decimal::serde::str")]
    pub realized_funding: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub realized_trading: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub unrealized_leg: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub fees: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub net: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub return_on_deployed_capital: Decimal,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortfolioDto {
    #[serde(with = "rust_decimal::serde::str")]
    pub equity: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub deployable_equity: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub deployed_capital: Decimal,
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
    pub async fn portfolio(
        &self,
        context: TenantContext,
        now: DateTime<Utc>,
    ) -> Result<PortfolioDto, sqlx::Error> {
        Ok(aggregate(&self.positions.list(context).await?, now))
    }
    pub async fn health(
        &self,
        context: TenantContext,
        now: DateTime<Utc>,
    ) -> Result<HealthDto, sqlx::Error> {
        let controls = self.breakers.controls(context).await?;
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
    pub async fn alerts(&self, context: TenantContext) -> Result<Vec<AlertDto>, sqlx::Error> {
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        let rows = sqlx::query(
            "SELECT payload FROM alerts WHERE tenant_id = $1 ORDER BY created_at DESC, id DESC",
        )
        .bind(context.tenant_id().as_uuid())
        .fetch_all(transaction.connection())
        .await?;
        let result = rows
            .iter()
            .map(|row| {
                serde_json::from_str(&row.get::<String, _>("payload"))
                    .map_err(|error| sqlx::Error::Protocol(error.to_string()))
            })
            .collect();
        transaction.commit().await?;
        result
    }
    pub async fn acknowledge(
        &self,
        context: TenantContext,
        id: &str,
        user: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<AlertDto>, sqlx::Error> {
        let id = Uuid::parse_str(id).map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        let Some(row) = sqlx::query("SELECT payload FROM alerts WHERE tenant_id = $1 AND id = $2")
            .bind(context.tenant_id().as_uuid())
            .bind(id)
            .fetch_optional(transaction.connection())
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
        sqlx::query("UPDATE alerts SET acknowledged_at = $3, acknowledged_by = $4, payload = $5 WHERE tenant_id = $1 AND id = $2").bind(context.tenant_id().as_uuid()).bind(id).bind(now).bind(user_id).bind(payload).execute(transaction.connection()).await?;
        transaction.commit().await?;
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
        let capital = position.reserved_capital;
        if position.state != "CLOSED" {
            deployed += capital;
        }
        *assets.entry(position.underlying.clone()).or_default() += capital;
        *venues.entry(position.long_leg.venue.clone()).or_default() += capital / Decimal::from(2);
        *venues.entry(position.short_leg.venue.clone()).or_default() += capital / Decimal::from(2);
        funding += position.funding_captured;
        let position_fees = position.long_leg.fees + position.short_leg.fees;
        fees += position_fees;
        if position.state == "CLOSED" {
            trading += position.realized_pnl - position.funding_captured + position_fees;
        } else {
            unrealized += position.long_leg.filled_qty
                * (position.long_leg.mark_price - position.long_leg.avg_fill_price)
                + position.short_leg.filled_qty
                    * (position.short_leg.avg_fill_price - position.short_leg.mark_price);
        }
    }
    let net = funding + trading + unrealized - fees;
    let allocation = |values: BTreeMap<String, Decimal>| {
        values
            .into_iter()
            .map(|(key, value)| AllocationDto {
                key,
                deployed_capital: value,
                fraction: if deployed.is_zero() {
                    Decimal::ZERO
                } else {
                    value / deployed
                },
            })
            .collect()
    };
    PortfolioDto {
        equity: reference + net,
        deployable_equity: (reference - deployed).max(Decimal::ZERO),
        deployed_capital: deployed,
        by_venue: allocation(venues),
        by_underlying: allocation(assets),
        pnl: PnlDto {
            realized_funding: funding,
            realized_trading: trading,
            unrealized_leg: unrealized,
            fees,
            net,
            return_on_deployed_capital: if deployed.is_zero() {
                Decimal::ZERO
            } else {
                net / deployed
            },
        },
        as_of: now.to_rfc3339(),
    }
}
