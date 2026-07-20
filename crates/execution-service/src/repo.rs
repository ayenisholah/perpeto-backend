//! Postgres persistence for paper positions.
//!
//! Each position is stored as a serialized `PositionDto` payload (text) so the
//! schema stays stable as the contract grows (mirroring the scanner repo). The
//! `idempotency_key` column is unique, so a repeated open with the same key
//! resolves to the existing row rather than a duplicate position.

use chrono::{DateTime, Utc};
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
        dto: &PositionDto,
        idempotency_key: &str,
    ) -> Result<(), sqlx::Error> {
        let payload = encode(dto)?;
        sqlx::query(
            "INSERT INTO positions (id, route_id, state, opened_at, idempotency_key, payload) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(parse_uuid(&dto.id)?)
        .bind(&dto.route_id)
        .bind(&dto.state)
        .bind(parse_time(&dto.opened_at)?)
        .bind(idempotency_key)
        .bind(&payload)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    /// Update an existing position's state and payload (e.g. on close).
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the update fails.
    pub async fn update(&self, dto: &PositionDto) -> Result<(), sqlx::Error> {
        let payload = encode(dto)?;
        sqlx::query("UPDATE positions SET state = $2, payload = $3 WHERE id = $1")
            .bind(parse_uuid(&dto.id)?)
            .bind(&dto.state)
            .bind(&payload)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    /// Read a position by the idempotency key it was opened under.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the query fails.
    pub async fn get_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<PositionDto>, sqlx::Error> {
        let row = sqlx::query("SELECT payload FROM positions WHERE idempotency_key = $1 LIMIT 1")
            .bind(idempotency_key)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => Ok(Some(decode(&row.get::<String, _>("payload"))?)),
            None => Ok(None),
        }
    }

    /// List all positions, newest first.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the query fails.
    pub async fn list(&self) -> Result<Vec<PositionDto>, sqlx::Error> {
        let rows = sqlx::query("SELECT payload FROM positions ORDER BY opened_at DESC, id DESC")
            .fetch_all(&self.pool)
            .await?;
        let mut positions = Vec::with_capacity(rows.len());
        for row in &rows {
            positions.push(decode(&row.get::<String, _>("payload"))?);
        }
        Ok(positions)
    }

    /// Read one position by id.
    ///
    /// # Errors
    /// Returns the underlying [`sqlx::Error`] if the query fails.
    pub async fn get(&self, id: &str) -> Result<Option<PositionDto>, sqlx::Error> {
        let uuid = parse_uuid(id)?;
        let row = sqlx::query("SELECT payload FROM positions WHERE id = $1 LIMIT 1")
            .bind(uuid)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(row) => Ok(Some(decode(&row.get::<String, _>("payload"))?)),
            None => Ok(None),
        }
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
