//! Tenant pilot-state read/write against a live Postgres database.
//!
//! Skipped unless `DATABASE_URL` is set (as in CI). Each test seeds a fresh
//! random tenant, so no cleanup or fixed-id coordination is required.

use funding_arb_auth_service::Repo;
use funding_arb_auth_service::error::AuthError;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect to test database"),
    )
}

async fn seed_tenant(pool: &PgPool) -> String {
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES (gen_random_uuid(), 'pilot-test') RETURNING id::text")
        .fetch_one(pool)
        .await
        .expect("insert tenant")
        .get::<String, _>("id")
}

async fn seed_deleted_tenant(pool: &PgPool) -> String {
    sqlx::query(
        "INSERT INTO tenants (id, display_name, state, deleted_at) \
         VALUES (gen_random_uuid(), 'pilot-test', 'DELETED', now()) RETURNING id::text",
    )
    .fetch_one(pool)
    .await
    .expect("insert deleted tenant")
    .get::<String, _>("id")
}

#[tokio::test]
async fn pilot_state_round_trips_through_the_arming_states() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let repo = Repo::new(pool.clone());
    let id = seed_tenant(&pool).await;

    assert_eq!(repo.pilot_state(&id).await.expect("read"), "PAPER_ONLY");
    repo.set_pilot_state(&id, "PILOT_ELIGIBLE")
        .await
        .expect("to eligible");
    assert_eq!(repo.pilot_state(&id).await.expect("read"), "PILOT_ELIGIBLE");
    repo.set_pilot_state(&id, "PILOT_ARMED")
        .await
        .expect("to armed");
    assert_eq!(repo.pilot_state(&id).await.expect("read"), "PILOT_ARMED");
}

#[tokio::test]
async fn deleted_tenant_state_is_immutable() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let repo = Repo::new(pool.clone());
    let id = seed_deleted_tenant(&pool).await;

    assert!(matches!(
        repo.set_pilot_state(&id, "PILOT_ELIGIBLE").await,
        Err(AuthError::NotFound)
    ));
}
