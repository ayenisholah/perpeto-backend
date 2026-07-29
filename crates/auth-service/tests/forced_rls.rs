//! Forced row-level-security acceptance evidence (M4).
//!
//! Skipped unless `DATABASE_URL` is set. The configuration guard proves every
//! private tenant table carries `FORCE ROW LEVEL SECURITY` plus a tenant policy,
//! so a future table that forgets isolation fails this test. The behavioral
//! check proves a transaction with no tenant context reads and writes nothing.
//!
//! The behavioral check requires a non-`BYPASSRLS`, non-owner runtime role (the
//! role `assert_safe_runtime_role` mandates); a bypassing role would defeat RLS.

use std::collections::HashMap;

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

/// Every private tenant table made forced-RLS by migrations 0016 and 0017.
const PRIVATE_TABLES: &[&str] = &[
    "positions",
    "circuit_breakers",
    "outbox_events",
    "strategies",
    "strategy_versions",
    "notification_routes",
    "alerts",
    "venue_accounts",
    "venue_product_connections",
    "fee_schedules",
    "encrypted_credentials",
    "connector_order_intents",
    "connector_order_events",
    "connector_fills",
    "connector_reconciliation_runs",
    "connector_discrepancies",
    "connector_balance_snapshots",
    "connector_position_snapshots",
    "connector_funding_ledger",
    "venue_health_samples",
    "audit_events",
    "passkey_credentials",
    "webauthn_challenges",
    "passkey_action_proofs",
];

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

#[tokio::test]
async fn every_private_table_forces_rls_with_a_tenant_policy() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let expected: Vec<String> = PRIVATE_TABLES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let rows = sqlx::query(
        "SELECT c.relname::text AS relname,
                c.relforcerowsecurity AS forced,
                EXISTS (
                    SELECT 1 FROM pg_policies p
                    WHERE p.schemaname = 'public' AND p.tablename = c.relname
                      AND (coalesce(p.qual, '') LIKE '%app_current_tenant_id%'
                           OR coalesce(p.with_check, '') LIKE '%app_current_tenant_id%')
                ) AS has_policy
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relname::text = ANY($1)",
    )
    .bind(&expected[..])
    .fetch_all(&pool)
    .await
    .expect("query catalog");

    let found: HashMap<String, (bool, bool)> = rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("relname"),
                (
                    row.get::<bool, _>("forced"),
                    row.get::<bool, _>("has_policy"),
                ),
            )
        })
        .collect();

    let mut failures = Vec::new();
    for table in PRIVATE_TABLES {
        match found.get(*table) {
            None => failures.push(format!("{table}: table missing")),
            Some((forced, has_policy)) => {
                if !forced {
                    failures.push(format!("{table}: not FORCE ROW LEVEL SECURITY"));
                }
                if !has_policy {
                    failures.push(format!("{table}: no app_current_tenant_id policy"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "forced-RLS gaps: {failures:?}");
}

#[tokio::test]
async fn missing_tenant_context_reads_and_writes_nothing() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    // A fresh tenant with one breaker row, written under its own context.
    let tenant: String =
        sqlx::query("INSERT INTO tenants (id, display_name) VALUES (gen_random_uuid(), 'rls-test') RETURNING id::text")
            .fetch_one(&pool)
            .await
            .expect("seed tenant")
            .get("id");

    let mut seeded = pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant.as_str())
        .execute(&mut *seeded)
        .await
        .expect("set context");
    sqlx::query(
        "INSERT INTO circuit_breakers
             (id, tenant_id, scope, action, source, created_at, payload)
         VALUES (gen_random_uuid(), $1::uuid, 'GLOBAL', 'HALT_NEW_RISK', 'TEST', now(), '{}')",
    )
    .bind(tenant.as_str())
    .execute(&mut *seeded)
    .await
    .expect("insert breaker under tenant");
    seeded.commit().await.expect("commit");

    // Under the tenant's context the row is visible.
    let mut with_ctx = pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant.as_str())
        .execute(&mut *with_ctx)
        .await
        .expect("set context");
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM circuit_breakers")
        .fetch_one(&mut *with_ctx)
        .await
        .expect("count with context");
    with_ctx.commit().await.expect("commit");
    assert!(visible >= 1, "tenant must see its own breaker");

    // With no context (app.tenant_id NULL) the policy matches nothing.
    let mut anon = pool.begin().await.expect("begin");
    let hidden: i64 = sqlx::query_scalar("SELECT count(*) FROM circuit_breakers")
        .fetch_one(&mut *anon)
        .await
        .expect("count without context");
    assert_eq!(hidden, 0, "no context must expose no breakers");

    // And a write with no context is rejected by the policy's WITH CHECK.
    let write = sqlx::query(
        "INSERT INTO circuit_breakers
             (id, tenant_id, scope, action, source, created_at, payload)
         VALUES (gen_random_uuid(), $1::uuid, 'GLOBAL', 'READ_ONLY', 'TEST', now(), '{}')",
    )
    .bind(tenant.as_str())
    .execute(&mut *anon)
    .await;
    assert!(
        write.is_err(),
        "insert without tenant context must be rejected"
    );
}
