//! Postgres round-trip tests for opened paper positions.
//!
//! Skipped unless `DATABASE_URL` is set (as it is in CI, which applies the
//! migrations and runs a Postgres service). The paper open itself is
//! deterministic, so these exercise persistence and idempotency, not fills.

use chrono::{TimeZone, Utc};
use funding_arb_execution_service::{ExecutionService, PositionDto, PositionsRepo};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;

// The tests share one table and clean up between scenarios, so serialize them.
static TEST_LOCK: Mutex<()> = Mutex::const_new(());

async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .ok()
}

async fn reset(pool: &PgPool) {
    sqlx::query("DELETE FROM positions")
        .execute(pool)
        .await
        .expect("clear positions");
}

fn first_route_id(service: &ExecutionService, now: chrono::DateTime<Utc>) -> String {
    service
        .scanner
        .scan(now)
        .expect("scan")
        .opportunities
        .first()
        .expect("at least one opportunity")
        .opportunity
        .route_id
        .clone()
}

#[tokio::test]
async fn create_then_read_round_trips() {
    let Some(pool) = pool().await else { return };
    let _guard = TEST_LOCK.lock().await;
    reset(&pool).await;

    let service = ExecutionService::default();
    let repo = PositionsRepo::new(pool.clone());
    let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
    let route_id = first_route_id(&service, now);

    let position = service
        .open(&route_id, None, "idem-key-1".into(), now)
        .expect("open");
    let dto = PositionDto::from_domain(&position);
    repo.create(&dto, "idem-key-1").await.expect("create");

    let by_key = repo
        .get_by_idempotency_key("idem-key-1")
        .await
        .expect("query");
    assert_eq!(by_key.expect("present").id, dto.id);

    let listed = repo.list().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].route_id, route_id);

    let fetched = repo.get(&dto.id).await.expect("get");
    assert_eq!(fetched.expect("present").state, "OPEN");
}

#[tokio::test]
async fn duplicate_idempotency_key_is_rejected() {
    let Some(pool) = pool().await else { return };
    let _guard = TEST_LOCK.lock().await;
    reset(&pool).await;

    let service = ExecutionService::default();
    let repo = PositionsRepo::new(pool.clone());
    let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
    let route_id = first_route_id(&service, now);

    let first = PositionDto::from_domain(
        &service
            .open(&route_id, None, "dup".into(), now)
            .expect("open"),
    );
    let second = PositionDto::from_domain(
        &service
            .open(&route_id, None, "dup".into(), now)
            .expect("open"),
    );
    repo.create(&first, "dup").await.expect("first insert");

    let error = repo
        .create(&second, "dup")
        .await
        .expect_err("duplicate key rejected");
    assert!(
        error
            .as_database_error()
            .is_some_and(|db| db.is_unique_violation())
    );
}

#[tokio::test]
async fn close_update_round_trips() {
    let Some(pool) = pool().await else { return };
    let _guard = TEST_LOCK.lock().await;
    reset(&pool).await;

    let service = ExecutionService::default();
    let repo = PositionsRepo::new(pool.clone());
    let opened = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
    let route_id = first_route_id(&service, opened);

    let position = service
        .open(&route_id, None, "idem-close".into(), opened)
        .expect("open");
    repo.create(&PositionDto::from_domain(&position), "idem-close")
        .await
        .expect("create");

    let closed = service.close(position, opened + chrono::Duration::hours(24));
    repo.update(&PositionDto::from_domain(&closed))
        .await
        .expect("update");

    let fetched = repo.get(&closed.id).await.expect("get").expect("present");
    assert_eq!(fetched.state, "CLOSED");
    assert!(fetched.funding_captured > 0.0);
    assert!(fetched.realized_pnl <= fetched.funding_captured);
}
