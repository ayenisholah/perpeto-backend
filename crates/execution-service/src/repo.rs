//! Postgres persistence for paper positions.
//!
//! Each position is stored as a serialized `PositionDto` payload (text) so the
//! schema stays stable as the contract grows (mirroring the scanner repo). The
//! `idempotency_key` column is unique, so a repeated open with the same key
//! resolves to the existing row rather than a duplicate position.

use chrono::{DateTime, Utc};
use funding_arb_tenant_context::{TenantContext, TenantTransaction};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::dto::PositionDto;

fn encode(dto: &PositionDto) -> Result<String, sqlx::Error> {
    serde_json::to_string(dto).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

fn decode(payload: &str) -> Result<PositionDto, sqlx::Error> {
    serde_json::from_str(payload).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

fn parse_uuid(value: &str) -> Result<Uuid, sqlx::Error> {
    Uuid::parse_str(value).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, sqlx::Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

/// Cloneable Postgres adapter for paper positions.
#[derive(Clone)]
pub struct PositionsRepo {
    pool: PgPool,
}

impl PositionsRepo {
    /// Build a repo over an existing connection pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Persist a newly opened position.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the insert fails (including a
    /// unique-violation on a duplicate `idempotency_key`).
    pub async fn create(
        &self,
        context: TenantContext,
        dto: &PositionDto,
        idempotency_key: &str,
    ) -> Result<(), sqlx::Error> {
        let payload = encode(dto)?;
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        sqlx::query(
            "INSERT INTO positions (tenant_id, id, route_id, state, opened_at, idempotency_key, payload) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(context.tenant_id().as_uuid())
        .bind(parse_uuid(&dto.id)?)
        .bind(&dto.route_id)
        .bind(&dto.state)
        .bind(parse_time(&dto.opened_at)?)
        .bind(idempotency_key)
        .bind(&payload)
        .execute(transaction.connection())
        .await?;
        transaction.commit().await
    }

    /// Update an existing position's state and payload (e.g. on close).
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the update fails.
    pub async fn update(
        &self,
        context: TenantContext,
        dto: &PositionDto,
    ) -> Result<(), sqlx::Error> {
        let payload = encode(dto)?;
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        sqlx::query(
            "UPDATE positions SET state = $3, payload = $4 WHERE tenant_id = $1 AND id = $2",
        )
        .bind(context.tenant_id().as_uuid())
        .bind(parse_uuid(&dto.id)?)
        .bind(&dto.state)
        .bind(&payload)
        .execute(transaction.connection())
        .await?;
        transaction.commit().await
    }

    /// Read a position by the idempotency key it was opened under.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the query fails.
    pub async fn get_by_idempotency_key(
        &self,
        context: TenantContext,
        idempotency_key: &str,
    ) -> Result<Option<PositionDto>, sqlx::Error> {
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        let row = sqlx::query(
            "SELECT payload FROM positions WHERE tenant_id = $1 AND idempotency_key = $2 LIMIT 1",
        )
        .bind(context.tenant_id().as_uuid())
        .bind(idempotency_key)
        .fetch_optional(transaction.connection())
        .await?;
        let result = match row {
            Some(row) => Ok(Some(decode(&row.get::<String, _>("payload"))?)),
            None => Ok(None),
        };
        transaction.commit().await?;
        result
    }

    /// List all positions, newest first.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the query fails.
    pub async fn list(&self, context: TenantContext) -> Result<Vec<PositionDto>, sqlx::Error> {
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        let rows = sqlx::query(
            "SELECT payload FROM positions WHERE tenant_id = $1 ORDER BY opened_at DESC, id DESC",
        )
        .bind(context.tenant_id().as_uuid())
        .fetch_all(transaction.connection())
        .await?;
        let mut positions = Vec::with_capacity(rows.len());
        for row in &rows {
            positions.push(decode(&row.get::<String, _>("payload"))?);
        }
        transaction.commit().await?;
        Ok(positions)
    }

    /// Read one position by id.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the query fails.
    pub async fn get(
        &self,
        context: TenantContext,
        id: &str,
    ) -> Result<Option<PositionDto>, sqlx::Error> {
        let uuid = parse_uuid(id)?;
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        let row =
            sqlx::query("SELECT payload FROM positions WHERE tenant_id = $1 AND id = $2 LIMIT 1")
                .bind(context.tenant_id().as_uuid())
                .bind(uuid)
                .fetch_optional(transaction.connection())
                .await?;
        let result = match row {
            Some(row) => Ok(Some(decode(&row.get::<String, _>("payload"))?)),
            None => Ok(None),
        };
        transaction.commit().await?;
        result
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
}
