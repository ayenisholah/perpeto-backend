//! Postgres persistence for opportunity scans.
//!
//! Each opportunity is stored as a serialized `OpportunityDto` payload (text) so
//! the schema stays stable as the contract grows; the driver needs no
//! compile-time schema (mirroring the auth repo) and the crate builds without a
//! live database.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::dto::{OpportunityDto, OpportunityScanDto};
use crate::market::{MarketSnapshot, MarketSnapshotDto};
use crate::service::ScanResult;

fn encode(dto: &OpportunityDto) -> Result<String, sqlx::Error> {
    serde_json::to_string(dto).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

fn decode(payload: &str) -> Result<OpportunityDto, sqlx::Error> {
    serde_json::from_str(payload).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

/// The most recent persisted market snapshot and when it was written.
#[derive(Clone, Debug)]
pub struct StoredSnapshot {
    /// The decoded snapshot.
    pub snapshot: MarketSnapshot,
    /// Wall-clock time the scan was written (`scan_runs.created_at`); the
    /// basis for staleness checks.
    pub recorded_at: DateTime<Utc>,
}

/// Cloneable Postgres adapter for scans and opportunities.
#[derive(Clone)]
pub struct ScannerRepo {
    pool: PgPool,
}

impl ScannerRepo {
    /// Build a repo over an existing connection pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Verify database connectivity.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the query fails.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    /// Persist a scan and its ranked opportunities in one transaction.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if any statement fails.
    pub async fn record_scan(&self, result: &ScanResult) -> Result<(), sqlx::Error> {
        self.record_scan_with_snapshot(result, None).await
    }

    /// Persist a scan, its ranked opportunities and, when given, the market
    /// snapshot it was ranked from, in one transaction.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if any statement fails.
    pub async fn record_scan_with_snapshot(
        &self,
        result: &ScanResult,
        snapshot: Option<&MarketSnapshot>,
    ) -> Result<(), sqlx::Error> {
        let scan_id = Uuid::new_v4();
        let payload = snapshot
            .map(|snapshot| serde_json::to_string(&MarketSnapshotDto::from_domain(snapshot)))
            .transpose()
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO scan_runs (id, scanned_at, model_version, market_snapshot) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(scan_id)
        .bind(result.scanned_at)
        .bind(result.model_version)
        .bind(payload)
        .execute(&mut *tx)
        .await?;

        for (position, ranked) in result.opportunities.iter().enumerate() {
            let dto = OpportunityDto::from_domain(&ranked.opportunity, &ranked.assessment);
            let payload = encode(&dto)?;
            sqlx::query(
                "INSERT INTO opportunities (scan_run_id, rank_position, route_id, payload) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(scan_id)
            .bind(i32::try_from(position).unwrap_or(i32::MAX))
            .bind(&dto.route_id)
            .bind(&payload)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// The most recently written market snapshot, or `None` if no scan has
    /// stored one.
    ///
    /// # Errors
    /// Returns [`sqlx::Error`] if the query fails or the payload cannot be
    /// decoded.
    pub async fn latest_snapshot(&self) -> Result<Option<StoredSnapshot>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT market_snapshot, created_at FROM scan_runs \
             WHERE market_snapshot IS NOT NULL ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let payload: String = row.get("market_snapshot");
        let dto: MarketSnapshotDto = serde_json::from_str(&payload)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let snapshot = dto
            .to_domain()
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        Ok(Some(StoredSnapshot {
            snapshot,
            recorded_at: row.get("created_at"),
        }))
    }

    async fn latest_scan_id(&self) -> Result<Option<(Uuid, DateTime<Utc>, String)>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, scanned_at, model_version FROM scan_runs \
             ORDER BY scanned_at DESC, id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| {
            (
                row.get("id"),
                row.get("scanned_at"),
                row.get("model_version"),
            )
        }))
    }

    /// Read the most recent scan as a wire DTO, or `None` if none exists.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if a query fails.
    pub async fn latest(&self) -> Result<Option<OpportunityScanDto>, sqlx::Error> {
        let Some((scan_id, scanned_at, model_version)) = self.latest_scan_id().await? else {
            return Ok(None);
        };
        let rows = sqlx::query(
            "SELECT payload FROM opportunities WHERE scan_run_id = $1 ORDER BY rank_position ASC",
        )
        .bind(scan_id)
        .fetch_all(&self.pool)
        .await?;

        let mut opportunities = Vec::with_capacity(rows.len());
        for row in &rows {
            let payload: String = row.get("payload");
            opportunities.push(decode(&payload)?);
        }
        Ok(Some(OpportunityScanDto {
            scanned_at: scanned_at.to_rfc3339(),
            model_version,
            opportunities,
        }))
    }

    /// Read one opportunity from the latest scan by route id.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if a query fails.
    pub async fn opportunity(&self, route_id: &str) -> Result<Option<OpportunityDto>, sqlx::Error> {
        let Some((scan_id, _, _)) = self.latest_scan_id().await? else {
            return Ok(None);
        };
        let row = sqlx::query(
            "SELECT payload FROM opportunities WHERE scan_run_id = $1 AND route_id = $2 LIMIT 1",
        )
        .bind(scan_id)
        .bind(route_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => {
                let payload: String = row.get("payload");
                Ok(Some(decode(&payload)?))
            }
            None => Ok(None),
        }
    }
}
