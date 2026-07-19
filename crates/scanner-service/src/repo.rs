//! Postgres persistence for opportunity scans.
//!
//! Numeric columns are `double precision` and timestamps `timestamptz`, so the
//! driver needs no compile-time schema (mirroring the auth repo) and the crate
//! builds without a live database.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::dto::{OpportunityDto, OpportunityScanDto};
use crate::service::ScanResult;

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
        sqlx::query("SELECT 1").execute(&self.pool).await.map(|_| ())
    }

    /// Persist a scan and its ranked opportunities in one transaction.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if any statement fails.
    pub async fn record_scan(&self, result: &ScanResult) -> Result<(), sqlx::Error> {
        let scan_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO scan_runs (id, scanned_at, model_version) VALUES ($1, $2, $3)")
            .bind(scan_id)
            .bind(result.scanned_at)
            .bind(result.model_version)
            .execute(&mut *tx)
            .await?;

        for (position, opportunity) in result.opportunities.iter().enumerate() {
            let dto = OpportunityDto::from_domain(opportunity);
            sqlx::query(
                "INSERT INTO opportunities (
                    scan_run_id, rank_position, route_id, route_type, underlying,
                    long_venue, short_venue, gross_funding_apr, entry_cost, exit_cost,
                    carry_cost, risk_buffer, net_apr, expected_net_pnl,
                    profitability_probability, available_capacity, next_funding_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)",
            )
            .bind(scan_id)
            .bind(i32::try_from(position).unwrap_or(i32::MAX))
            .bind(&dto.route_id)
            .bind(&dto.route_type)
            .bind(&dto.underlying)
            .bind(&dto.long_venue)
            .bind(&dto.short_venue)
            .bind(dto.gross_funding_apr)
            .bind(dto.entry_cost)
            .bind(dto.exit_cost)
            .bind(dto.carry_cost)
            .bind(dto.risk_buffer)
            .bind(dto.net_apr)
            .bind(dto.expected_net_pnl)
            .bind(dto.profitability_probability)
            .bind(dto.available_capacity)
            .bind(opportunity.next_funding_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// Read the most recent scan as a wire DTO, or `None` if none exists.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if a query fails.
    pub async fn latest(&self) -> Result<Option<OpportunityScanDto>, sqlx::Error> {
        let Some(run) = sqlx::query(
            "SELECT id, scanned_at, model_version FROM scan_runs \
             ORDER BY scanned_at DESC, id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let scan_id: Uuid = run.get("id");
        let scanned_at: DateTime<Utc> = run.get("scanned_at");
        let model_version: String = run.get("model_version");

        let rows = sqlx::query(
            "SELECT route_id, route_type, underlying, long_venue, short_venue, \
                    gross_funding_apr, entry_cost, exit_cost, carry_cost, risk_buffer, \
                    net_apr, expected_net_pnl, profitability_probability, available_capacity, \
                    next_funding_at \
             FROM opportunities WHERE scan_run_id = $1 ORDER BY rank_position ASC",
        )
        .bind(scan_id)
        .fetch_all(&self.pool)
        .await?;

        let opportunities = rows
            .iter()
            .map(|row| {
                let next_funding_at: DateTime<Utc> = row.get("next_funding_at");
                OpportunityDto {
                    route_id: row.get("route_id"),
                    route_type: row.get("route_type"),
                    underlying: row.get("underlying"),
                    long_venue: row.get("long_venue"),
                    short_venue: row.get("short_venue"),
                    gross_funding_apr: row.get("gross_funding_apr"),
                    entry_cost: row.get("entry_cost"),
                    exit_cost: row.get("exit_cost"),
                    carry_cost: row.get("carry_cost"),
                    risk_buffer: row.get("risk_buffer"),
                    net_apr: row.get("net_apr"),
                    expected_net_pnl: row.get("expected_net_pnl"),
                    profitability_probability: row.get("profitability_probability"),
                    available_capacity: row.get("available_capacity"),
                    next_funding_at: next_funding_at.to_rfc3339(),
                }
            })
            .collect();

        Ok(Some(OpportunityScanDto {
            scanned_at: scanned_at.to_rfc3339(),
            model_version,
            opportunities,
        }))
    }
}
