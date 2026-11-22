//! Paper portfolio, health, and alert read models.

use crate::{BreakersRepo, PositionDto, PositionsRepo};
use chrono::{DateTime, Utc};
use funding_arb_domain::market::Venue;
use funding_arb_scanner_service::fixtures::MarketStatus;
use funding_arb_scanner_service::{FeedStatus, StoredSnapshot};
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
    /// `HEALTHY`, or `DEGRADED` when a venue breaker is active or the feed is
    /// not healthy.
    pub status: String,
    /// Whether the latest snapshot reached the venue.
    pub connected: bool,
    /// Seconds since the venue's newest observation; when the venue has none,
    /// seconds since the snapshot that tried to read it (0 with no snapshot).
    pub market_data_age_seconds: i64,
    pub clock_drift_ms: i64,
    /// Public feed health from the latest market snapshot: `HEALTHY`,
    /// `DEGRADED`, `UNAVAILABLE`, or `UNKNOWN` when no snapshot covers it.
    pub feed_status: String,
    /// RFC 3339 time of the venue's newest observation, if any.
    pub last_observed_at: Option<String>,
    /// Why the feed is unavailable, when it is.
    pub feed_error: Option<String>,
}

/// One venue's health from its breakers and the latest stored market
/// snapshot (DEC-0016). Fixture snapshots carry no feed reports, so a venue
/// present in the legs is judged by the legs' status instead.
#[must_use]
pub fn venue_health(
    venue: Venue,
    breaker_active: bool,
    market: Option<&StoredSnapshot>,
    now: DateTime<Utc>,
) -> VenueHealthDto {
    let (feed_status, observed_at, feed_error) = match market {
        None => ("UNKNOWN", None, None),
        Some(stored) => {
            let snapshot = &stored.snapshot;
            let newest_leg = snapshot
                .legs
                .iter()
                .filter(|leg| leg.observation.instrument.venue == venue)
                .map(|leg| leg.observation.observed_at)
                .max();
            match snapshot.venues.iter().find(|feed| feed.venue == venue) {
                Some(feed) => (
                    feed.status.as_str(),
                    feed.observed_at.or(newest_leg),
                    feed.error.clone(),
                ),
                None => {
                    let legs: Vec<_> = snapshot
                        .legs
                        .iter()
                        .filter(|leg| leg.observation.instrument.venue == venue)
                        .collect();
                    if legs.is_empty() {
                        ("UNKNOWN", None, None)
                    } else if legs.iter().all(|leg| leg.status == MarketStatus::Healthy) {
                        (FeedStatus::Healthy.as_str(), newest_leg, None)
                    } else {
                        (FeedStatus::Degraded.as_str(), newest_leg, None)
                    }
                }
            }
        }
    };
    let age_basis = observed_at.or_else(|| market.map(|stored| stored.recorded_at));
    let market_data_age_seconds = age_basis.map_or(0, |at| (now - at).num_seconds().max(0));
    let feed_healthy = feed_status == FeedStatus::Healthy.as_str();
    VenueHealthDto {
        venue: venue.as_str().into(),
        status: if breaker_active || !feed_healthy {
            "DEGRADED".into()
        } else {
            "HEALTHY".into()
        },
        connected: matches!(feed_status, "HEALTHY" | "DEGRADED"),
        market_data_age_seconds,
        clock_drift_ms: 0,
        feed_status: feed_status.into(),
        last_observed_at: observed_at.map(|at| at.to_rfc3339()),
        feed_error,
    }
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
    /// Venue, service and feed health for one tenant. Venue health combines
    /// the tenant's venue breakers with the latest stored market snapshot.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if a query fails.
    pub async fn health(
        &self,
        context: TenantContext,
        market: Option<&StoredSnapshot>,
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
        let venues = [Venue::Binance, Venue::Bybit, Venue::Okx]
            .into_iter()
            .map(|venue| {
                let breaker_active = controls
                    .breakers
                    .iter()
                    .any(|breaker| breaker.scope_key.as_deref() == Some(venue.as_str()));
                venue_health(venue, breaker_active, market, now)
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

#[cfg(test)]
mod tests {
    use super::venue_health;
    use chrono::{DateTime, TimeZone, Utc};
    use funding_arb_domain::market::Venue;
    use funding_arb_scanner_service::fixtures::{self, MarketStatus};
    use funding_arb_scanner_service::{
        FeedStatus, MarketSnapshot, StoredSnapshot, VenueFeedStatus,
    };

    fn at(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, second).unwrap()
    }

    fn stored(snapshot: MarketSnapshot, recorded: u32) -> StoredSnapshot {
        StoredSnapshot {
            snapshot,
            recorded_at: at(recorded),
        }
    }

    fn feed(
        venue: Venue,
        status: FeedStatus,
        observed: Option<u32>,
        error: Option<&str>,
    ) -> VenueFeedStatus {
        VenueFeedStatus {
            venue,
            status,
            observed_at: observed.map(at),
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn no_snapshot_is_unknown_and_disconnected() {
        let health = venue_health(Venue::Binance, false, None, at(30));
        assert_eq!(health.feed_status, "UNKNOWN");
        assert_eq!(health.status, "DEGRADED");
        assert!(!health.connected);
        assert_eq!(health.market_data_age_seconds, 0);
        assert_eq!(health.last_observed_at, None);
    }

    #[test]
    fn a_healthy_live_feed_reports_its_real_age() {
        let mut snapshot = MarketSnapshot::from_legs(at(0), Vec::new());
        snapshot
            .venues
            .push(feed(Venue::Okx, FeedStatus::Healthy, Some(10), None));
        let health = venue_health(Venue::Okx, false, Some(&stored(snapshot, 12)), at(40));
        assert_eq!(health.feed_status, "HEALTHY");
        assert_eq!(health.status, "HEALTHY");
        assert!(health.connected);
        assert_eq!(health.market_data_age_seconds, 30);
        assert_eq!(health.last_observed_at, Some(at(10).to_rfc3339()));
    }

    #[test]
    fn an_unavailable_feed_is_disconnected_with_its_error_and_attempt_age() {
        let mut snapshot = MarketSnapshot::from_legs(at(0), Vec::new());
        snapshot.venues.push(feed(
            Venue::Bybit,
            FeedStatus::Unavailable,
            None,
            Some("health timed out"),
        ));
        let health = venue_health(Venue::Bybit, false, Some(&stored(snapshot, 20)), at(50));
        assert_eq!(health.feed_status, "UNAVAILABLE");
        assert_eq!(health.status, "DEGRADED");
        assert!(!health.connected);
        assert_eq!(health.feed_error.as_deref(), Some("health timed out"));
        assert_eq!(health.market_data_age_seconds, 30);
    }

    #[test]
    fn a_breaker_degrades_an_otherwise_healthy_venue() {
        let mut snapshot = MarketSnapshot::from_legs(at(0), Vec::new());
        snapshot
            .venues
            .push(feed(Venue::Binance, FeedStatus::Healthy, Some(0), None));
        let health = venue_health(Venue::Binance, true, Some(&stored(snapshot, 0)), at(1));
        assert_eq!(health.status, "DEGRADED");
        assert_eq!(health.feed_status, "HEALTHY");
        assert!(health.connected);
    }

    #[test]
    fn fixture_snapshots_are_judged_by_their_legs() {
        let now = at(0);
        let legs = fixtures::load_at(now).expect("fixtures");
        let venue = legs[0].observation.instrument.venue;
        let healthy = venue_health(
            venue,
            false,
            Some(&stored(MarketSnapshot::from_legs(now, legs.clone()), 0)),
            now,
        );
        let expected = if legs
            .iter()
            .filter(|leg| leg.observation.instrument.venue == venue)
            .all(|leg| leg.status == MarketStatus::Healthy)
        {
            "HEALTHY"
        } else {
            "DEGRADED"
        };
        assert_eq!(healthy.feed_status, expected);
        assert!(healthy.connected);

        let mut degraded = legs;
        for leg in &mut degraded {
            leg.status = MarketStatus::Degraded;
        }
        let health = venue_health(
            venue,
            false,
            Some(&stored(MarketSnapshot::from_legs(now, degraded), 0)),
            now,
        );
        assert_eq!(health.feed_status, "DEGRADED");
        assert_eq!(health.status, "DEGRADED");
    }

    #[test]
    fn a_venue_absent_from_a_fixture_snapshot_is_unknown() {
        let snapshot = MarketSnapshot::from_legs(at(0), Vec::new());
        let health = venue_health(Venue::Okx, false, Some(&stored(snapshot, 5)), at(15));
        assert_eq!(health.feed_status, "UNKNOWN");
        assert!(!health.connected);
        assert_eq!(health.market_data_age_seconds, 10);
    }
}
