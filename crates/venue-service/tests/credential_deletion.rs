//! Credential deletion against a live Postgres database.
//!
//! Skipped unless `DATABASE_URL` is set (as in CI). Uses the transitional AES
//! `SecretStore` so no Vault is required; each run seeds a fresh random tenant.
//! Verifies that deleting a venue account destroys every stored secret, makes the
//! account invisible to all read paths, and is idempotent (a repeat delete 404s).

use funding_arb_tenant_context::{TenantContext, TenantId};
use funding_arb_venue_service::{
    CredentialBundle, CredentialEnrollmentService, EncryptedPostgresSecretStore, EnrollRequest,
    EnrollmentError, SecretContext, SecretStore, VenueAccountsRepo,
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
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Deletion test')")
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

fn request(alias: &str) -> EnrollRequest {
    EnrollRequest {
        venue: "BYBIT".to_owned(),
        alias: alias.to_owned(),
        product: PRODUCT.to_owned(),
        environment: "TESTNET".to_owned(),
    }
}

#[tokio::test]
async fn delete_destroys_secrets_tombstones_and_is_idempotent() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tenant = seed_tenant(&pool).await;
    let service = service(&pool);
    let accounts = VenueAccountsRepo::new(pool.clone());

    let account = service
        .enroll(
            tenant,
            &request("deletable"),
            &CredentialBundle {
                api_key: "del-key".to_owned(),
                api_secret: "del-secret".to_owned(),
                passphrase: None,
            },
        )
        .await
        .expect("enroll");
    let account_id = account.id.clone();
    let secret_ref = Uuid::parse_str(
        account.connections[0]
            .secret_ref
            .as_ref()
            .expect("secret_ref"),
    )
    .expect("secret uuid");
    let ctx = SecretContext {
        tenant,
        venue_account_id: Uuid::parse_str(&account_id).expect("account uuid"),
        venue: "BYBIT".to_owned(),
        product: PRODUCT.to_owned(),
    };
    // The secret exists before deletion.
    assert!(store(&pool).load(secret_ref, &ctx).await.is_ok());

    service.delete(tenant, &account_id).await.expect("delete");

    // The secret is destroyed, and the account is invisible to every read path.
    assert!(store(&pool).load(secret_ref, &ctx).await.is_err());
    assert!(
        accounts
            .get(tenant, &account_id)
            .await
            .expect("get after delete")
            .is_none(),
        "a tombstoned account must not be readable"
    );
    assert!(
        accounts
            .list(tenant)
            .await
            .expect("list after delete")
            .is_empty(),
        "a tombstoned account must not be listed"
    );

    // Deletion is idempotent: the account is already gone, so a repeat is NotFound.
    assert_eq!(
        service.delete(tenant, &account_id).await,
        Err(EnrollmentError::NotFound),
    );
}
