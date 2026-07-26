//! Credential enrollment lifecycle against a live Postgres database.
//!
//! Skipped unless `DATABASE_URL` is set (as in CI). Uses the transitional AES
//! `SecretStore` so no Vault is required; each run seeds a fresh random tenant,
//! so no cleanup or fixed-id coordination with other test binaries is needed.

use funding_arb_tenant_context::{TenantContext, TenantId};
use funding_arb_venue_service::{
    CredentialBundle, CredentialEnrollmentService, EncryptedPostgresSecretStore, EnrollRequest,
    SecretContext, SecretStore, VenueAccountsRepo,
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
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Enrollment test')")
        .bind(id)
        .execute(pool)
        .await
        .expect("insert tenant");
    TenantContext::new(TenantId::new(id))
}

fn service(pool: &PgPool) -> CredentialEnrollmentService {
    CredentialEnrollmentService::new(
        pool.clone(),
        Box::new(EncryptedPostgresSecretStore::new(pool.clone(), KEY, 1)),
    )
}

fn store(pool: &PgPool) -> EncryptedPostgresSecretStore {
    EncryptedPostgresSecretStore::new(pool.clone(), KEY, 1)
}

fn credential(api_key: &str) -> CredentialBundle {
    CredentialBundle {
        api_key: api_key.to_owned(),
        api_secret: "test-secret".to_owned(),
        passphrase: None,
    }
}

fn context(tenant: TenantContext, account_id: &str) -> SecretContext {
    SecretContext {
        tenant,
        venue_account_id: Uuid::parse_str(account_id).expect("account uuid"),
        venue: "BYBIT".to_owned(),
        product: PRODUCT.to_owned(),
    }
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
async fn enroll_stores_secret_then_revoke_destroys_it() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tenant = seed_tenant(&pool).await;
    let service = service(&pool);

    let account = service
        .enroll(tenant, &request("primary"), &credential("enroll-key"))
        .await
        .expect("enroll");
    let connection = &account.connections[0];
    assert_eq!(connection.status, "PENDING_PREFLIGHT");
    // The masked account returned to callers never carries credential material.
    let rendered = serde_json::to_string(&account).expect("serialize account");
    assert!(
        !rendered.contains("test-secret"),
        "masked account must not leak the api secret"
    );
    let secret_ref = Uuid::parse_str(connection.secret_ref.as_ref().expect("secret_ref"))
        .expect("secret uuid");

    // The credential is retrievable under the owning tenant/account context.
    let ctx = context(tenant, &account.id);
    let loaded = store(&pool).load(secret_ref, &ctx).await.expect("load");
    assert_eq!(loaded.api_key, "enroll-key");

    // A different tenant cannot read it — the query is tenant-scoped and the row
    // is forced-RLS to the owner.
    let other = seed_tenant(&pool).await;
    let other_ctx = SecretContext {
        tenant: other,
        ..ctx.clone()
    };
    assert!(store(&pool).load(secret_ref, &other_ctx).await.is_err());

    // Revoke clears the reference, disables the connection, and destroys the key.
    let revoked = service
        .revoke(tenant, &account.id, PRODUCT)
        .await
        .expect("revoke");
    assert!(revoked.connections[0].secret_ref.is_none());
    assert_eq!(revoked.connections[0].status, "DISABLED");
    assert!(store(&pool).load(secret_ref, &ctx).await.is_err());
}

#[tokio::test]
async fn venue_accounts_are_tenant_isolated() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tenant = seed_tenant(&pool).await;
    let service = service(&pool);
    let account = service
        .enroll(tenant, &request("isolated"), &credential("iso-key"))
        .await
        .expect("enroll");

    let accounts = VenueAccountsRepo::new(pool.clone());
    // The owning tenant sees the account; another tenant sees neither it nor any.
    assert!(
        accounts
            .get(tenant, &account.id)
            .await
            .expect("owner get")
            .is_some()
    );
    let other = seed_tenant(&pool).await;
    assert!(
        accounts
            .get(other, &account.id)
            .await
            .expect("cross-tenant get")
            .is_none()
    );
    assert!(
        accounts
            .list(other)
            .await
            .expect("cross-tenant list")
            .is_empty()
    );
}

#[tokio::test]
async fn rotate_replaces_the_stored_credential() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tenant = seed_tenant(&pool).await;
    let service = service(&pool);

    let account = service
        .enroll(tenant, &request("rotator"), &credential("old-key"))
        .await
        .expect("enroll");
    let secret_ref =
        Uuid::parse_str(account.connections[0].secret_ref.as_ref().expect("secret_ref"))
            .expect("secret uuid");

    let rotated = service
        .rotate(tenant, &account.id, PRODUCT, &credential("new-key"))
        .await
        .expect("rotate");
    // Rotation resets eligibility.
    assert_eq!(rotated.connections[0].status, "PENDING_PREFLIGHT");

    let loaded = store(&pool)
        .load(secret_ref, &context(tenant, &account.id))
        .await
        .expect("load");
    assert_eq!(loaded.api_key, "new-key");
}
