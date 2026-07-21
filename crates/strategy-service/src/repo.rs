use funding_arb_tenant_context::{TenantContext, TenantTransaction};
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

    pub async fn list(&self, context: TenantContext) -> Result<Vec<StrategyDto>, sqlx::Error> {
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        let rows = sqlx::query(
            "SELECT payload FROM strategies WHERE tenant_id = $1 ORDER BY created_at, id",
        )
        .bind(context.tenant_id().as_uuid())
        .fetch_all(transaction.connection())
        .await?;
        let result = rows
            .iter()
            .map(|row| decode(&row.get::<String, _>("payload")))
            .collect();
        transaction.commit().await?;
        result
    }

    pub async fn get(
        &self,
        context: TenantContext,
        id: &str,
    ) -> Result<Option<StrategyDto>, sqlx::Error> {
        let id = parse(id)?;
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        let row = sqlx::query("SELECT payload FROM strategies WHERE tenant_id = $1 AND id = $2")
            .bind(context.tenant_id().as_uuid())
            .bind(id)
            .fetch_optional(transaction.connection())
            .await?;
        let result = row
            .map(|value| decode(&value.get::<String, _>("payload")))
            .transpose();
        transaction.commit().await?;
        result
    }

    pub async fn create(
        &self,
        context: TenantContext,
        dto: &StrategyDto,
    ) -> Result<(), sqlx::Error> {
        let payload = encode(dto)?;
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        sqlx::query("INSERT INTO strategies (tenant_id, id, name, enabled, current_version, created_at, payload) VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(context.tenant_id().as_uuid()).bind(parse(&dto.id)?).bind(&dto.name).bind(dto.enabled).bind(i32::try_from(dto.current.version).unwrap_or(i32::MAX))
            .bind(parse_time(&dto.current.created_at)?).bind(&payload).execute(transaction.connection()).await?;
        insert_version(transaction.connection(), context, dto).await?;
        transaction.commit().await
    }

    pub async fn update(
        &self,
        context: TenantContext,
        dto: &StrategyDto,
    ) -> Result<(), sqlx::Error> {
        let payload = encode(dto)?;
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        sqlx::query("UPDATE strategies SET name = $3, enabled = $4, current_version = $5, payload = $6 WHERE tenant_id = $1 AND id = $2")
            .bind(context.tenant_id().as_uuid()).bind(parse(&dto.id)?).bind(&dto.name).bind(dto.enabled).bind(i32::try_from(dto.current.version).unwrap_or(i32::MAX)).bind(payload).execute(transaction.connection()).await?;
        insert_version(transaction.connection(), context, dto).await?;
        transaction.commit().await
    }

    pub async fn delete(&self, context: TenantContext, id: &str) -> Result<u64, sqlx::Error> {
        let mut transaction = TenantTransaction::begin(&self.pool, context).await?;
        let affected = sqlx::query("DELETE FROM strategies WHERE tenant_id = $1 AND id = $2")
            .bind(context.tenant_id().as_uuid())
            .bind(parse(id)?)
            .execute(transaction.connection())
            .await?
            .rows_affected();
        transaction.commit().await?;
        Ok(affected)
    }
}

async fn insert_version(
    connection: &mut sqlx::PgConnection,
    context: TenantContext,
    dto: &StrategyDto,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO strategy_versions (tenant_id, strategy_id, version, created_at, payload) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING")
        .bind(context.tenant_id().as_uuid()).bind(parse(&dto.id)?).bind(i32::try_from(dto.current.version).unwrap_or(i32::MAX)).bind(parse_time(&dto.current.created_at)?).bind(encode(dto)?)
        .execute(connection).await.map(|_| ())
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
