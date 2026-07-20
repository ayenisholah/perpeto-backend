use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::StrategyDto;

/// Serialized-payload strategy repository with immutable version rows.
#[derive(Clone)]
pub struct StrategiesRepo {
    pool: PgPool,
}

impl StrategiesRepo {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<StrategyDto>, sqlx::Error> {
        let rows = sqlx::query("SELECT payload FROM strategies ORDER BY created_at, id")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| decode(&row.get::<String, _>("payload")))
            .collect()
    }

    pub async fn get(&self, id: &str) -> Result<Option<StrategyDto>, sqlx::Error> {
        let id = parse(id)?;
        let row = sqlx::query("SELECT payload FROM strategies WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|value| decode(&value.get::<String, _>("payload")))
            .transpose()
    }

    pub async fn create(&self, dto: &StrategyDto) -> Result<(), sqlx::Error> {
        let payload = encode(dto)?;
        sqlx::query("INSERT INTO strategies (id, name, enabled, current_version, created_at, payload) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(parse(&dto.id)?).bind(&dto.name).bind(dto.enabled).bind(i32::try_from(dto.current.version).unwrap_or(i32::MAX))
            .bind(parse_time(&dto.current.created_at)?).bind(&payload).execute(&self.pool).await?;
        self.insert_version(dto).await
    }

    pub async fn update(&self, dto: &StrategyDto) -> Result<(), sqlx::Error> {
        let payload = encode(dto)?;
        sqlx::query("UPDATE strategies SET name = $2, enabled = $3, current_version = $4, payload = $5 WHERE id = $1")
            .bind(parse(&dto.id)?).bind(&dto.name).bind(dto.enabled).bind(i32::try_from(dto.current.version).unwrap_or(i32::MAX)).bind(payload).execute(&self.pool).await?;
        self.insert_version(dto).await
    }

    async fn insert_version(&self, dto: &StrategyDto) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO strategy_versions (strategy_id, version, created_at, payload) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
            .bind(parse(&dto.id)?).bind(i32::try_from(dto.current.version).unwrap_or(i32::MAX)).bind(parse_time(&dto.current.created_at)?).bind(encode(dto)?)
            .execute(&self.pool).await.map(|_| ())
    }

    pub async fn delete(&self, id: &str) -> Result<u64, sqlx::Error> {
        Ok(sqlx::query("DELETE FROM strategies WHERE id = $1")
            .bind(parse(id)?)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }
}

fn parse(value: &str) -> Result<Uuid, sqlx::Error> {
    Uuid::parse_str(value).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}
fn parse_time(value: &str) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&chrono::Utc))
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))
}
fn encode(value: &StrategyDto) -> Result<String, sqlx::Error> {
    serde_json::to_string(value).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}
fn decode(value: &str) -> Result<StrategyDto, sqlx::Error> {
    serde_json::from_str(value).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}
