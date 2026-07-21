//! Paper monitoring worker: heartbeats and deduplicated outbox alerts.

use chrono::Utc;
use funding_arb_execution_service::AlertDto;
use funding_arb_tenant_context::{
    TenantContext, TenantId, TenantTransaction, assert_safe_runtime_role,
};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::{env, process::ExitCode, time::Duration};
use uuid::Uuid;

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("worker runtime failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run())
}

async fn run() -> ExitCode {
    let Ok(url) = env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL is required");
        return ExitCode::FAILURE;
    };
    let pool = match PgPoolOptions::new().max_connections(3).connect(&url).await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("worker database failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = assert_safe_runtime_role(&pool).await {
        eprintln!("unsafe runtime database role: {error}");
        return ExitCode::FAILURE;
    }
    println!("Perpeto M2 monitoring worker started");
    loop {
        if let Err(error) = heartbeat(&pool).await {
            eprintln!("heartbeat failed: {error}");
        }
        match active_tenants(&pool).await {
            Ok(tenants) => {
                for tenant in tenants {
                    if let Err(error) = drain_outbox(&pool, tenant).await {
                        eprintln!("tenant alert projection failed: {error}");
                    }
                }
            }
            Err(error) => eprintln!("tenant dispatch failed: {error}"),
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

async fn heartbeat(pool: &PgPool) -> Result<(), sqlx::Error> {
    let now = Utc::now();
    for service in ["worker", "api", "engine"] {
        let payload = serde_json::json!({"service": service, "status": "HEALTHY", "observed_at": now.to_rfc3339()}).to_string();
        sqlx::query("INSERT INTO service_heartbeats (service, status, observed_at, payload) VALUES ($1, 'HEALTHY', $2, $3) ON CONFLICT DO NOTHING").bind(service).bind(now).bind(payload).execute(pool).await?;
    }
    Ok(())
}

async fn drain_outbox(pool: &PgPool, context: TenantContext) -> Result<(), sqlx::Error> {
    let mut transaction = TenantTransaction::begin(pool, context).await?;
    let rows = sqlx::query("SELECT id, event_type, aggregate_id, payload, created_at FROM outbox_events WHERE tenant_id = $1 AND completed_at IS NULL AND available_at <= NOW() ORDER BY created_at LIMIT 50 FOR UPDATE SKIP LOCKED")
        .bind(context.tenant_id().as_uuid())
        .fetch_all(transaction.connection()).await?;
    for row in rows {
        let event_id: Uuid = row.get("id");
        let event_type: String = row.get("event_type");
        let aggregate_id: String = row.get("aggregate_id");
        let created_at: chrono::DateTime<Utc> = row.get("created_at");
        let alert = AlertDto {
            id: Uuid::new_v4().to_string(),
            dedup_key: format!("{event_type}:{aggregate_id}"),
            severity: if event_type.contains("CIRCUIT_BREAKER") {
                "CRITICAL".into()
            } else {
                "INFORMATIONAL".into()
            },
            title: event_type.replace('_', " "),
            message: format!("Paper event for {aggregate_id}"),
            created_at: created_at.to_rfc3339(),
            acknowledged_at: None,
            acknowledged_by: None,
        };
        let payload = serde_json::to_string(&alert)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        sqlx::query("INSERT INTO alerts (tenant_id, id, dedup_key, severity, created_at, payload) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (tenant_id, dedup_key) DO NOTHING").bind(context.tenant_id().as_uuid()).bind(Uuid::parse_str(&alert.id).map_err(|error| sqlx::Error::Protocol(error.to_string()))?).bind(&alert.dedup_key).bind(&alert.severity).bind(created_at).bind(payload).execute(transaction.connection()).await?;
        sqlx::query(
            "UPDATE outbox_events SET completed_at = NOW(), attempts = attempts + 1 WHERE tenant_id = $1 AND id = $2",
        )
        .bind(context.tenant_id().as_uuid())
        .bind(event_id)
        .execute(transaction.connection())
        .await?;
    }
    transaction.commit().await
}

async fn active_tenants(pool: &PgPool) -> Result<Vec<TenantContext>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id FROM tenants WHERE state NOT IN ('SUSPENDED', 'DELETED') ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| TenantContext::new(TenantId::new(row.get("id"))))
        .collect())
}
