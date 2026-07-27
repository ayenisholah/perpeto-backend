//! Ciphertext rewrap against a live Postgres database (AES backend).
//!
//! Skipped unless `DATABASE_URL` is set (as in CI). Proves the transitional AES
//! store can migrate a stored secret from an older key version to the current
//! one without changing the credential: the key version bumps, the ciphertext
//! changes, the `plaintext_digest` stays identical, the credential still
//! decrypts under the new key, and a second rewrap is a no-op.

use funding_arb_tenant_context::{TenantContext, TenantId, TenantTransaction};
use funding_arb_venue_service::{
    CredentialBundle, EncryptedPostgresSecretStore, SecretContext, SecretStore,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const KEY_V1: [u8; 32] = [1_u8; 32];
const KEY_V2: [u8; 32] = [2_u8; 32];
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
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Rewrap test')")
        .bind(id)
        .execute(pool)
        .await
        .expect("insert tenant");
    TenantContext::new(TenantId::new(id))
}

/// Read the forced-RLS credential row for the given secret inside a tenant
/// transaction: (key_version, ciphertext, plaintext_digest).
async fn read_row(pool: &PgPool, tenant: TenantContext, secret_ref: Uuid) -> (i32, Vec<u8>, Vec<u8>) {
    let mut transaction = TenantTransaction::begin(pool, tenant)
        .await
        .expect("begin tenant tx");
    let row = sqlx::query_as::<_, (i32, Vec<u8>, Vec<u8>)>(
        "SELECT key_version, ciphertext, plaintext_digest
         FROM encrypted_credentials WHERE tenant_id = $1 AND secret_ref = $2",
    )
    .bind(tenant.tenant_id().as_uuid())
    .bind(secret_ref)
    .fetch_one(transaction.connection())
    .await
    .expect("read credential row");
    transaction.commit().await.expect("commit");
    row
}

#[tokio::test]
async fn rewrap_migrates_key_version_without_changing_the_credential() {
    let Some(pool) = pool().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let tenant = seed_tenant(&pool).await;
    let context = SecretContext {
        tenant,
        venue_account_id: Uuid::new_v4(),
        venue: "BYBIT".to_owned(),
        product: PRODUCT.to_owned(),
    };
    let credential = CredentialBundle {
        api_key: "rewrap-key".to_owned(),
        api_secret: "rewrap-secret".to_owned(),
        passphrase: Some("phrase".to_owned()),
    };

    // Store under key version 1.
    let v1 = EncryptedPostgresSecretStore::new(pool.clone(), KEY_V1, 1);
    let stored = v1.put(&context, &credential).await.expect("put v1");
    let secret_ref = stored.secret_ref;
    let (version_before, ciphertext_before, digest_before) = read_row(&pool, tenant, secret_ref).await;
    assert_eq!(version_before, 1);

    // Rewrap with a store whose current key is v2 but which can still decrypt v1.
    let v2 = EncryptedPostgresSecretStore::with_previous_keys(
        pool.clone(),
        KEY_V2,
        2,
        vec![(1, KEY_V1)],
    );
    let rewrapped = v2.rewrap(tenant, secret_ref).await.expect("rewrap to v2");
    assert_eq!(rewrapped.key_version, 2);

    let (version_after, ciphertext_after, digest_after) = read_row(&pool, tenant, secret_ref).await;
    assert_eq!(version_after, 2, "key version must advance");
    assert_ne!(ciphertext_after, ciphertext_before, "ciphertext must be re-encrypted");
    assert_eq!(digest_after, digest_before, "the credential (and its digest) must be unchanged");

    // The credential still decrypts under the new key.
    let loaded = v2.load(secret_ref, &context).await.expect("load after rewrap");
    assert_eq!(loaded.api_key, credential.api_key);
    assert_eq!(loaded.api_secret, credential.api_secret);
    assert_eq!(loaded.passphrase, credential.passphrase);

    // A second rewrap is a no-op: already at the current version, ciphertext stable.
    let again = v2.rewrap(tenant, secret_ref).await.expect("second rewrap");
    assert_eq!(again.key_version, 2);
    let (_, ciphertext_final, _) = read_row(&pool, tenant, secret_ref).await;
    assert_eq!(ciphertext_final, ciphertext_after, "a no-op rewrap must not rewrite the row");
}
