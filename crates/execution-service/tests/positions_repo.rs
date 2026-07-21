//! Postgres round-trip tests for opened paper positions.
//!
//! Skipped unless `DATABASE_URL` is set (as it is in CI, which applies the
//! migrations and runs a Postgres service). The paper open itself is
//! deterministic, so these exercise persistence and idempotency, not fills.

use chrono::{TimeZone, Utc};
use funding_arb_execution_service::{ExecutionService, PositionDto, PositionsRepo};
use funding_arb_tenant_context::{TenantContext, TenantId, TenantTransaction};
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

fn tenant(raw: &str) -> TenantContext {
    TenantContext::new(TenantId::new(
        uuid::Uuid::parse_str(raw).expect("tenant uuid"),
    ))
}

async fn prepare_tenants(pool: &PgPool) -> (TenantContext, TenantContext) {
    let first = tenant("10000000-0000-0000-0000-000000000001");
    let second = tenant("20000000-0000-0000-0000-000000000002");
    for (context, name) in [(first, "Test tenant A"), (second, "Test tenant B")] {
        sqlx::query("INSERT INTO tenants (id, display_name, state) VALUES ($1, $2, 'ACTIVE') ON CONFLICT (id) DO NOTHING")
            .bind(context.tenant_id().as_uuid())
            .bind(name)
            .execute(pool)
            .await
            .expect("prepare tenant");
        let mut transaction = TenantTransaction::begin(pool, context)
            .await
            .expect("tenant transaction");
        sqlx::query("DELETE FROM positions WHERE tenant_id = $1")
            .bind(context.tenant_id().as_uuid())
            .execute(transaction.connection())
            .await
            .expect("clear positions");
        transaction.commit().await.expect("commit reset");
    }
    (first, second)
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
    let (tenant, _) = prepare_tenants(&pool).await;

    let service = ExecutionService::default();
    let repo = PositionsRepo::new(pool.clone());
    let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
    let route_id = first_route_id(&service, now);

    let position = service
        .open(&route_id, None, "idem-key-1".into(), now)
        .expect("open");
    let dto = PositionDto::from_domain(&position);
    repo.create(tenant, &dto, "idem-key-1")
        .await
        .expect("create");

    let by_key = repo
        .get_by_idempotency_key(tenant, "idem-key-1")
        .await
        .expect("query");
    assert_eq!(by_key.expect("present").id, dto.id);

    let listed = repo.list(tenant).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].route_id, route_id);

    let fetched = repo.get(tenant, &dto.id).await.expect("get");
    assert_eq!(fetched.expect("present").state, "OPEN");
}

#[tokio::test]
async fn duplicate_idempotency_key_is_rejected() {
    let Some(pool) = pool().await else { return };
    let _guard = TEST_LOCK.lock().await;
    let (tenant, _) = prepare_tenants(&pool).await;

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
    repo.create(tenant, &first, "dup")
        .await
        .expect("first insert");

    let error = repo
        .create(tenant, &second, "dup")
        .await
        .expect_err("duplicate key rejected");
    assert!(
        error
            .as_database_error()
            .is_some_and(|db| db.is_unique_violation())
    );
}

#[tokio::test]
async fn tenant_rows_are_isolated_and_idempotency_is_tenant_scoped() {
    let Some(pool) = pool().await else { return };
    let _guard = TEST_LOCK.lock().await;
    let (first_tenant, second_tenant) = prepare_tenants(&pool).await;
    let service = ExecutionService::default();
    let repo = PositionsRepo::new(pool);
    let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
    let route_id = first_route_id(&service, now);
    let first = PositionDto::from_domain(
        &service
            .open(&route_id, None, "shared-key".into(), now)
            .expect("first open"),
    );
    let second = PositionDto::from_domain(
        &service
            .open(&route_id, None, "shared-key".into(), now)
            .expect("second open"),
    );

    repo.create(first_tenant, &first, "shared-key")
        .await
        .expect("first tenant insert");
    repo.create(second_tenant, &second, "shared-key")
        .await
        .expect("same key is valid in another tenant");

    assert_eq!(repo.list(first_tenant).await.expect("first list").len(), 1);
    assert_eq!(
        repo.list(second_tenant).await.expect("second list").len(),
        1
    );
    assert!(
        repo.get(first_tenant, &second.id)
            .await
            .expect("cross tenant lookup")
            .is_none()
    );
    assert!(
        repo.get(second_tenant, &first.id)
            .await
            .expect("cross tenant lookup")
            .is_none()
    );
}

#[tokio::test]
async fn close_update_round_trips() {
    let Some(pool) = pool().await else { return };
    let _guard = TEST_LOCK.lock().await;
    let (tenant, _) = prepare_tenants(&pool).await;

    let service = ExecutionService::default();
    let repo = PositionsRepo::new(pool.clone());
    let opened = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
    let route_id = first_route_id(&service, opened);

    let position = service
        .open(&route_id, None, "idem-close".into(), opened)
        .expect("open");
    repo.create(tenant, &PositionDto::from_domain(&position), "idem-close")
        .await
        .expect("create");

    let closed = service.close(position, opened + chrono::Duration::hours(24));
    repo.update(tenant, &PositionDto::from_domain(&closed))
        .await
        .expect("update");

    let fetched = repo
        .get(tenant, &closed.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(fetched.state, "CLOSED");
    assert!(fetched.funding_captured > 0.0);
    assert!(fetched.realized_pnl <= fetched.funding_captured);
}

#[tokio::test]
async fn supervision_updates_round_trip_across_the_scenario() {
    let Some(pool) = pool().await else { return };
    let _guard = TEST_LOCK.lock().await;
    let (tenant, _) = prepare_tenants(&pool).await;

    let service = ExecutionService::default();
    let repo = PositionsRepo::new(pool.clone());
    let t0 = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
    let t1 = Utc.with_ymd_and_hms(2026, 7, 19, 4, 0, 0).unwrap();
    let t2 = Utc.with_ymd_and_hms(2026, 7, 19, 8, 0, 0).unwrap();
    let t3 = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
    let position = service
        .open(
            "PERP_PERP:BINANCE>BYBIT:BTC",
            None,
            "idem-supervise".into(),
            t0,
        )
        .expect("open");
    repo.create(
        tenant,
        &PositionDto::from_domain(&position),
        "idem-supervise",
    )
    .await
    .expect("create");

    let mut current = position;
    for now in [t1, t2, t3] {
        current = service.supervise(current, now).expect("supervise");
        repo.update(tenant, &PositionDto::from_domain(&current))
            .await
            .expect("update");
        current = repo
            .get(tenant, &current.id)
            .await
            .expect("get")
            .expect("present")
            .to_domain();
    }

    let fetched = repo
        .get(tenant, &current.id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(fetched.state, "CLOSED");
    assert_eq!(fetched.exit_reason.as_deref(), Some("FUNDING_FLIP"));
    assert!(
        fetched
            .long_leg
            .orders
            .iter()
            .any(|order| order.client_order_id.contains("-hedge-long-")),
    );
}
