//! Cross-tenant isolation for circuit breakers (M4 acceptance evidence).
//!
//! Skipped unless `DATABASE_URL` is set. Each run seeds two fresh random
//! tenants, so no cleanup or fixed-id coordination is required.

use chrono::Utc;
use funding_arb_execution_service::{BreakerAction, BreakerScope, BreakersRepo};
use funding_arb_tenant_context::{TenantContext, TenantId};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

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

async fn seed_tenant(pool: &PgPool) -> TenantContext {
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'breaker-test')")
        .bind(id)
        .execute(pool)
        .await
        .expect("insert tenant");
    TenantContext::new(TenantId::new(id))
}

#[tokio::test]
async fn breakers_are_isolated_and_resolve_is_tenant_scoped() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let first = seed_tenant(&pool).await;
    let second = seed_tenant(&pool).await;
    let repo = BreakersRepo::new(pool.clone());
    let now = Utc::now();

    repo.activate(
        first,
        BreakerScope::Global,
        None,
        BreakerAction::HaltNewRisk,
        "OPERATOR",
        "isolation test".to_owned(),
        None,
        now,
    )
    .await
    .expect("activate under first tenant");

    // The first tenant sees its breaker; the second sees nothing.
    assert!(
        repo.controls(first)
            .await
            .expect("first controls")
            .effective_action
            .is_some()
    );
    assert!(
        repo.controls(second)
            .await
            .expect("second controls")
            .effective_action
            .is_none(),
        "second tenant must not see the first tenant's breaker"
    );

    // Resolving operator breakers in the second tenant clears none of the first's.
    let cleared = repo
        .resolve_operator(second, now)
        .await
        .expect("resolve in second tenant");
    assert_eq!(cleared, 0, "second tenant has nothing to resolve");
    assert!(
        repo.controls(first)
            .await
            .expect("first still active")
            .effective_action
            .is_some(),
        "the first tenant's breaker survives another tenant's resolve"
    );
}
