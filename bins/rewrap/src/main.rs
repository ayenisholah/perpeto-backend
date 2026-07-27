//! Scheduled ciphertext-rewrap worker (M4 Slice 5/6).
//!
//! Implements the audited rewrap job from `docs/KEY_MANAGEMENT.md`: on a fixed
//! interval it re-encrypts every non-revoked stored credential to the secret
//! backend's current key version via [`SecretStore::rewrap`], so a rotated key
//! can be retired without touching the exchange credential. Idempotent — a
//! secret already at the current version is a no-op — and plaintext never leaves
//! the store (Vault rewraps ciphertext-only; the AES store re-encrypts
//! internally).
//!
//! Like the reconciler, it requires SANDBOX or SHADOW (the secret backend is
//! only configured outside PAPER) and refuses PAPER/LIVE.

use std::{env, process::ExitCode, str::FromStr, time::Duration};

use funding_arb_domain::TradingMode;
use funding_arb_tenant_context::{
    TenantContext, TenantId, TenantTransaction, assert_safe_runtime_role,
};
use funding_arb_venue_service::{SecretStoreError, secret_store_from_env};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::main]
async fn main() -> ExitCode {
    let mode = env::var("PERPETO_MODE").unwrap_or_else(|_| "PAPER".into());
    let mode = match TradingMode::from_str(&mode) {
        Ok(TradingMode::Sandbox | TradingMode::Shadow) => mode,
        _ => {
            eprintln!("rewrap worker requires SANDBOX or SHADOW mode");
            return ExitCode::FAILURE;
        }
    };
    let database_url = match env::var("DATABASE_URL") {
        Ok(value) => value,
        Err(_) => {
            eprintln!("DATABASE_URL is required");
            return ExitCode::FAILURE;
        }
    };
    let pool = match PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
    {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to connect to database: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = assert_safe_runtime_role(&pool).await {
        eprintln!("unsafe rewrap database role: {error}");
        return ExitCode::FAILURE;
    }
    let interval = env::var("PERPETO_REWRAP_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(3600);
    let store = match secret_store_from_env(pool.clone()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    println!("Perpeto credential rewrap worker started in {mode} mode");
    loop {
        let tenants = match active_tenants(&pool).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("failed to load active tenants: {error}");
                return ExitCode::FAILURE;
            }
        };
        let mut processed = 0_u64;
        let mut failed = 0_u64;
        for tenant in tenants {
            let refs = match active_secret_refs(&pool, tenant).await {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("failed to list credentials: {error}");
                    continue;
                }
            };
            for secret_ref in refs {
                match store.rewrap(tenant, secret_ref).await {
                    Ok(_) => processed += 1,
                    // A concurrently revoked/deleted row is not an error worth failing on.
                    Err(SecretStoreError::NotFound) => {}
                    Err(error) => {
                        failed += 1;
                        eprintln!("rewrap failed for {secret_ref}: {error}");
                    }
                }
            }
        }
        println!("rewrap cycle complete: {processed} processed, {failed} failed");
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

async fn active_tenants(pool: &sqlx::PgPool) -> Result<Vec<TenantContext>, sqlx::Error> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tenants WHERE state NOT IN ('SUSPENDED', 'DELETED') ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .map(|ids| {
        ids.into_iter()
            .map(|id| TenantContext::new(TenantId::new(id)))
            .collect()
    })
}

/// The `encrypted_credentials` table is forced-RLS, so enumeration must run
/// inside a tenant transaction that sets `app_current_tenant_id()`.
async fn active_secret_refs(
    pool: &sqlx::PgPool,
    tenant: TenantContext,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut transaction = TenantTransaction::begin(pool, tenant).await?;
    let refs = sqlx::query_scalar::<_, Uuid>(
        "SELECT secret_ref FROM encrypted_credentials
         WHERE tenant_id = $1 AND revoked_at IS NULL ORDER BY secret_ref",
    )
    .bind(tenant.tenant_id().as_uuid())
    .fetch_all(transaction.connection())
    .await?;
    transaction.commit().await?;
    Ok(refs)
}
