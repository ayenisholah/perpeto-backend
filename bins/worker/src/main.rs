//! Paper monitoring worker: heartbeats and deduplicated outbox alerts.

use chrono::Utc;
use funding_arb_execution_service::AlertDto;
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
    println!("Perpeto M2 monitoring worker started");
    loop {
        if let Err(error) = heartbeat(&pool).await {
            eprintln!("heartbeat failed: {error}");
        }
        if let Err(error) = drain_outbox(&pool).await {
            eprintln!("alert projection failed: {error}");
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

async fn drain_outbox(pool: &PgPool) -> Result<(), sqlx::Error> {
    let rows = sqlx::query("SELECT id, event_type, aggregate_id, payload, created_at FROM outbox_events WHERE completed_at IS NULL AND available_at <= NOW() ORDER BY created_at LIMIT 50").fetch_all(pool).await?;
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
        sqlx::query("INSERT INTO alerts (id, dedup_key, severity, created_at, payload) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (dedup_key) DO NOTHING").bind(Uuid::parse_str(&alert.id).map_err(|error| sqlx::Error::Protocol(error.to_string()))?).bind(&alert.dedup_key).bind(&alert.severity).bind(created_at).bind(payload).execute(pool).await?;
        sqlx::query(
            "UPDATE outbox_events SET completed_at = NOW(), attempts = attempts + 1 WHERE id = $1",
        )
        .bind(event_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}
