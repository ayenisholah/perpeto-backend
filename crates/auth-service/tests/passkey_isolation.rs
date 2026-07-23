//! Adversarial cross-tenant isolation for passkey storage (migration `0017`).
//!
//! Skipped unless `DATABASE_URL` is set (as in CI, which runs Postgres). The
//! test drives the transaction-local tenant context the same way
//! `funding_arb_tenant_context::TenantTransaction` does — `set_config`
//! `app.tenant_id` — so it exercises the forced row-level-security policy
//! directly, without needing a WebAuthn authenticator.
//!
//! CI must connect as a non-`BYPASSRLS`, non-owner runtime role (the same role
//! `assert_safe_runtime_role` requires); a bypassing role would defeat the
//! policy and this test would (correctly) fail.

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

// Fixed identifiers so the test cleans up exactly its own rows and never
// truncates tables other integration binaries may be using concurrently.
const TENANT_A: &str = "aaaaaaaa-0000-0000-0000-000000000001";
const TENANT_B: &str = "bbbbbbbb-0000-0000-0000-000000000002";
const USER_A: &str = "aaaaaaaa-0000-0000-0000-0000000000a1";
const USER_B: &str = "bbbbbbbb-0000-0000-0000-0000000000b2";
const CREDENTIAL_ID: &[u8] = b"passkey-isolation-credential";

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

/// Seed two independent personal tenants, their owners, and memberships.
async fn seed(pool: &PgPool) {
    // Remove any residue from a prior run (children first).
    sqlx::query("DELETE FROM passkey_credentials WHERE tenant_id = ANY($1::uuid[])")
        .bind([TENANT_A, TENANT_B])
        .execute(pool)
        .await
        .expect("cleanup credentials");
    sqlx::query("DELETE FROM tenant_memberships WHERE user_id = ANY($1::uuid[])")
        .bind([USER_A, USER_B])
        .execute(pool)
        .await
        .expect("cleanup memberships");
    sqlx::query("DELETE FROM tenants WHERE id = ANY($1::uuid[])")
        .bind([TENANT_A, TENANT_B])
        .execute(pool)
        .await
        .expect("cleanup tenants");
    sqlx::query("DELETE FROM users WHERE id = ANY($1::uuid[])")
        .bind([USER_A, USER_B])
        .execute(pool)
        .await
        .expect("cleanup users");

    for (tenant, user) in [(TENANT_A, USER_A), (TENANT_B, USER_B)] {
        sqlx::query("INSERT INTO users (id, status) VALUES ($1::uuid, 'ACTIVE')")
            .bind(user)
            .execute(pool)
            .await
            .expect("insert user");
        sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1::uuid, 'Isolation test')")
            .bind(tenant)
            .execute(pool)
            .await
            .expect("insert tenant");
        sqlx::query(
            "INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1::uuid, $2::uuid)",
        )
        .bind(tenant)
        .bind(user)
        .execute(pool)
        .await
        .expect("insert membership");
    }
}

/// Count visible passkey rows inside a transaction bound to `tenant`.
async fn count_visible(pool: &PgPool, tenant: &str) -> i64 {
    let mut transaction = pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant)
        .execute(&mut *transaction)
        .await
        .expect("set tenant context");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM passkey_credentials")
        .fetch_one(&mut *transaction)
        .await
        .expect("count");
    transaction.commit().await.expect("commit");
    count
}

#[tokio::test]
async fn passkeys_are_isolated_per_tenant() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    seed(&pool).await;

    // Register a passkey for tenant A within A's transaction-local context.
    let mut transaction = pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(TENANT_A)
        .execute(&mut *transaction)
        .await
        .expect("set tenant context");
    sqlx::query(
        "INSERT INTO passkey_credentials
             (id, tenant_id, user_id, credential_id, authenticator, sign_count, label)
         VALUES (gen_random_uuid(), $1::uuid, $2::uuid, $3, $4, 0, 'Test key')",
    )
    .bind(TENANT_A)
    .bind(USER_A)
    .bind(CREDENTIAL_ID)
    .bind(b"{}".as_slice())
    .execute(&mut *transaction)
    .await
    .expect("insert passkey under tenant A");
    transaction.commit().await.expect("commit");

    // Tenant A sees its own passkey; tenant B sees nothing; a missing context
    // (NULL app.tenant_id) also matches no row.
    assert_eq!(count_visible(&pool, TENANT_A).await, 1, "A must see its passkey");
    assert_eq!(count_visible(&pool, TENANT_B).await, 0, "B must not see A's passkey");

    let mut anon = pool.begin().await.expect("begin");
    let anon_count: i64 = sqlx::query_scalar("SELECT count(*) FROM passkey_credentials")
        .fetch_one(&mut *anon)
        .await
        .expect("count without context");
    anon.commit().await.expect("commit");
    assert_eq!(anon_count, 0, "no tenant context must expose no passkeys");
}

#[tokio::test]
async fn cross_tenant_insert_is_rejected() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    seed(&pool).await;

    // Under tenant B's context, attempting to write a row owned by tenant A must
    // violate the RLS WITH CHECK clause and fail.
    let mut transaction = pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(TENANT_B)
        .execute(&mut *transaction)
        .await
        .expect("set tenant context");
    let result = sqlx::query(
        "INSERT INTO passkey_credentials
             (id, tenant_id, user_id, credential_id, authenticator, sign_count, label)
         VALUES (gen_random_uuid(), $1::uuid, $2::uuid, $3, $4, 0, 'Forged key')",
    )
    .bind(TENANT_A)
    .bind(USER_A)
    .bind(b"forged".as_slice())
    .bind(b"{}".as_slice())
    .execute(&mut *transaction)
    .await;
    assert!(result.is_err(), "writing another tenant's row must be rejected");
}
