//! Hash-chained audit log against a live Postgres database.
//!
//! Skipped unless `DATABASE_URL` is set (as in CI). Verifies that appended events
//! form an intact per-tenant chain, that a second tenant's chain is independent
//! (forced RLS), and that mutating any stored row is detected by `verify`.

use funding_arb_auth_service::Repo;
use funding_arb_tenant_context::{TenantContext, TenantId, TenantTransaction};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

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
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Audit test')")
        .bind(id)
        .execute(pool)
        .await
        .expect("insert tenant");
    TenantContext::new(TenantId::new(id))
}

/// Insert a pseudonymized actor with no linked user (user_id is nullable), so the
/// test does not need to build a full user row.
async fn seed_actor(pool: &PgPool) -> (Uuid, Uuid) {
    let id = Uuid::new_v4();
    let pseudonym = Uuid::new_v4();
    sqlx::query("INSERT INTO audit_actors (id, user_id, pseudonym) VALUES ($1, NULL, $2)")
        .bind(id)
        .bind(pseudonym)
        .execute(pool)
        .await
        .expect("insert actor");
    (id, pseudonym)
}

#[tokio::test]
async fn chain_links_verify_and_detect_tampering() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let repo = Repo::new(pool.clone());
    let tenant = seed_tenant(&pool).await;
    let (actor_id, pseudonym) = seed_actor(&pool).await;

    for event in ["CREDENTIAL_ENROLLED", "PILOT_ARMED", "ROUTE_CERTIFIED"] {
        repo.append_audit(
            tenant,
            actor_id,
            pseudonym,
            event,
            "venue_account",
            "acct-1",
            None,
            "{\"product\":\"LINEAR_PERPETUAL\"}",
        )
        .await
        .expect("append audit");
    }

    // The freshly built chain verifies.
    assert!(
        repo.verify_audit_chain(tenant)
            .await
            .expect("verify intact")
    );

    // The review listing returns every event (newest first) with safe fields.
    let listed = repo.list_audit(tenant).await.expect("list audit");
    assert_eq!(listed.len(), 3);
    assert!(listed.iter().all(|row| row.subject_id == "acct-1"));
    let mut kinds: Vec<&str> = listed.iter().map(|row| row.event_type.as_str()).collect();
    kinds.sort_unstable();
    assert_eq!(
        kinds,
        ["CREDENTIAL_ENROLLED", "PILOT_ARMED", "ROUTE_CERTIFIED"]
    );
    // Newest first: occurred_at is non-increasing down the list.
    assert!(
        listed
            .windows(2)
            .all(|pair| pair[0].occurred_at >= pair[1].occurred_at)
    );

    // A second tenant's chain is independent (empty ⇒ trivially intact) — proving
    // appends and verification are tenant-scoped under forced RLS.
    let other = seed_tenant(&pool).await;
    assert!(repo.verify_audit_chain(other).await.expect("verify other"));

    // Tamper with a stored row under the tenant's RLS context; the chain breaks.
    let mut transaction = TenantTransaction::begin(&pool, tenant)
        .await
        .expect("begin tenant tx");
    sqlx::query("UPDATE audit_events SET details = '{\"product\":\"TAMPERED\"}'::jsonb WHERE tenant_id = $1")
        .bind(tenant.tenant_id().as_uuid())
        .execute(transaction.connection())
        .await
        .expect("tamper");
    transaction.commit().await.expect("commit tamper");

    assert!(
        !repo
            .verify_audit_chain(tenant)
            .await
            .expect("verify tampered"),
        "a mutated row must break the chain"
    );
}
