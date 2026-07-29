//! Scheduled-preflight demotion against a live Postgres database.
//!
//! Skipped unless `DATABASE_URL` is set (as in CI). Exercises the shared
//! [`PreflightService`] the worker runs: an `ELIGIBLE` connection whose declared
//! config has drifted (withdrawal permission re-enabled) is fail-closed demoted
//! to `PREFLIGHT_FAILED` without any venue reconciliation — the config half fails
//! first, so no network is touched. Each run seeds a fresh random tenant.

use std::sync::Arc;

use funding_arb_domain::TradingMode;
use funding_arb_tenant_context::{TenantContext, TenantId};
use funding_arb_venue_service::{
    ConnectorRegistry, CredentialBundle, CredentialEnrollmentService, EncryptedPostgresSecretStore,
    EnrollRequest, PreflightService, ReconciliationService, VenueAccountsRepo,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const KEY: [u8; 32] = [7_u8; 32];
const PRODUCT: &str = "LINEAR_PERPETUAL";

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
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Preflight worker test')")
        .bind(id)
        .execute(pool)
        .await
        .expect("insert tenant");
    TenantContext::new(TenantId::new(id))
}

fn enrollment(pool: &PgPool) -> CredentialEnrollmentService {
    CredentialEnrollmentService::new(
        pool.clone(),
        Box::new(EncryptedPostgresSecretStore::new(pool.clone(), KEY, 1)),
    )
}

fn preflight_service(pool: &PgPool) -> PreflightService {
    PreflightService::new(
        Arc::new(ConnectorRegistry::new(pool.clone(), KEY)),
        ReconciliationService::new(pool.clone()),
        Arc::new(enrollment(pool)),
    )
}

fn request(alias: &str) -> EnrollRequest {
    EnrollRequest {
        venue: "BYBIT".to_owned(),
        alias: alias.to_owned(),
        product: PRODUCT.to_owned(),
        environment: "TESTNET".to_owned(),
    }
}

#[tokio::test]
async fn preflight_demotes_eligible_route_on_withdrawal_drift() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tenant = seed_tenant(&pool).await;
    let enrollment = enrollment(&pool);
    let accounts = VenueAccountsRepo::new(pool.clone());

    // Enroll and drive the route to ELIGIBLE (as an operator preflight would).
    let account = enrollment
        .enroll(
            tenant,
            &request("drifter"),
            &CredentialBundle {
                api_key: "pf-key".to_owned(),
                api_secret: "pf-secret".to_owned(),
                passphrase: None,
            },
        )
        .await
        .expect("enroll");
    let account_id = account.id.clone();
    let eligible = enrollment
        .record_preflight_result(tenant, &account_id, PRODUCT, true)
        .await
        .expect("mark eligible");
    assert_eq!(eligible.connections[0].status, "ELIGIBLE");

    // Drift: the venue key regains withdrawal permission. The config half now
    // fails, so the worker's shared PreflightService must demote fail-closed
    // (and never reach reconciliation — no network in this test).
    let mut drifted = eligible.connections[0].clone();
    drifted.withdrawal_permission = Some(true);
    accounts
        .update_connection(tenant, &account_id, &drifted)
        .await
        .expect("persist drift");

    let account = accounts
        .get(tenant, &account_id)
        .await
        .expect("reload account")
        .expect("account exists");
    let outcome = preflight_service(&pool)
        .run(tenant, TradingMode::Sandbox, &account, PRODUCT)
        .await
        .expect("run preflight");
    assert_eq!(outcome.status, "PREFLIGHT_FAILED");
    assert!(
        outcome
            .checks
            .iter()
            .any(|check| check == "WITHDRAWAL_ENABLED"),
        "the withdrawal-permission drift must be the recorded failure: {:?}",
        outcome.checks
    );
    assert!(
        outcome.reconciliation.is_none(),
        "reconciliation must not run once the config half has failed"
    );

    // The demotion is persisted, not just reported.
    let reloaded = accounts
        .get(tenant, &account_id)
        .await
        .expect("reload")
        .expect("account exists");
    assert_eq!(reloaded.connections[0].status, "PREFLIGHT_FAILED");
}
