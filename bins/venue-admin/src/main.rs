use std::{env, error::Error, io::Read};

use chrono::Utc;
use funding_arb_tenant_context::{TenantContext, TenantId};
use funding_arb_venue_service::{
    CertificationState, CredentialBundle, ProductConnectionDto, ReconciliationState, SecretContext,
    SecretStore, VenueAccountDto, VenueAccountsRepo, secret_store_from_env,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;
use zeroize::Zeroizing;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let database_url = env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await?;
    let accounts = VenueAccountsRepo::new(pool.clone());
    let tenant = quarantine_tenant()?;

    match arguments.first().map(String::as_str) {
        Some("import") => {
            let store = secret_store_from_env(pool.clone())?;
            import(tenant, &accounts, store.as_ref(), &arguments[1..]).await?;
        }
        Some("rotate") => {
            let store = secret_store_from_env(pool.clone())?;
            rotate(tenant, store.as_ref(), &arguments[1..]).await?;
        }
        Some("disable") => disable(tenant, &accounts, &arguments[1..]).await?,
        Some("inspect") => inspect(tenant, &accounts).await?,
        _ => return Err(usage().into()),
    }
    Ok(())
}

async fn import(
    tenant: TenantContext,
    accounts: &VenueAccountsRepo,
    store: &dyn SecretStore,
    arguments: &[String],
) -> Result<(), Box<dyn Error>> {
    if arguments.len() != 4 {
        return Err(usage().into());
    }
    let venue = canonical_venue(&arguments[0])?;
    let alias = arguments[1].trim();
    if alias.is_empty() {
        return Err("account alias cannot be empty".into());
    }
    let product = canonical_product(&arguments[2])?;
    let environment = canonical_environment(venue, &arguments[3])?;
    let credential = read_credential(venue)?;
    let account_id = Uuid::new_v4();
    let connection_id = Uuid::new_v4();
    let connection = ProductConnectionDto {
        id: connection_id.to_string(),
        product: product.into(),
        environment: environment.into(),
        secret_ref: None,
        read_permission: true,
        trade_permission: environment != "PRODUCTION",
        withdrawal_permission: Some(false),
        status: "PENDING_TEST".into(),
        last_tested_at: None,
    };
    let mut account = VenueAccountDto {
        id: account_id.to_string(),
        venue: venue.into(),
        alias: alias.into(),
        masked_identity: mask_key(&credential.api_key),
        enabled: true,
        certification_state: CertificationState::PaperOnly,
        reconciliation_state: ReconciliationState::Pending,
        connections: vec![connection],
        last_reconciled_at: None,
        created_at: Utc::now().to_rfc3339(),
        tombstoned_at: None,
    };
    accounts.create(tenant, &account).await?;
    let context = SecretContext {
        tenant,
        venue_account_id: account_id,
        venue: venue.into(),
        product: product.into(),
    };
    let metadata = store.put(&context, &credential).await?;
    account.connections[0].secret_ref = Some(metadata.secret_ref.to_string());
    accounts
        .update_connection(tenant, &account.id, &account.connections[0])
        .await?;
    accounts.update(tenant, &account).await?;
    println!(
        "imported account {} ({venue}/{product}/{environment})",
        account.id
    );
    Ok(())
}

async fn rotate(
    tenant: TenantContext,
    store: &dyn SecretStore,
    arguments: &[String],
) -> Result<(), Box<dyn Error>> {
    if arguments.len() != 4 {
        return Err(usage().into());
    }
    let secret_ref = Uuid::parse_str(&arguments[0])?;
    let venue_account_id = Uuid::parse_str(&arguments[1])?;
    let venue = canonical_venue(&arguments[2])?;
    let product = canonical_product(&arguments[3])?;
    let credential = read_credential(venue)?;
    let context = SecretContext {
        tenant,
        venue_account_id,
        venue: venue.into(),
        product: product.into(),
    };
    store.rotate(secret_ref, &context, &credential).await?;
    println!("rotated credential {secret_ref}");
    Ok(())
}

async fn disable(
    tenant: TenantContext,
    accounts: &VenueAccountsRepo,
    arguments: &[String],
) -> Result<(), Box<dyn Error>> {
    if arguments.len() != 1 {
        return Err(usage().into());
    }
    let mut account = accounts
        .get(tenant, &arguments[0])
        .await?
        .ok_or("venue account not found")?;
    account.enabled = false;
    account.certification_state = CertificationState::Disabled;
    for connection in &mut account.connections {
        connection.status = "DISABLED".into();
        accounts
            .update_connection(tenant, &account.id, connection)
            .await?;
    }
    accounts.update(tenant, &account).await?;
    println!("disabled account {}", account.id);
    Ok(())
}

async fn inspect(
    tenant: TenantContext,
    accounts: &VenueAccountsRepo,
) -> Result<(), Box<dyn Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&accounts.list(tenant).await?)?
    );
    Ok(())
}

fn quarantine_tenant() -> Result<TenantContext, uuid::Error> {
    Uuid::parse_str("00000000-0000-0000-0000-000000000001")
        .map(TenantId::new)
        .map(TenantContext::new)
}

fn read_credential(venue: &str) -> Result<CredentialBundle, Box<dyn Error>> {
    let mut raw = Zeroizing::new(String::new());
    std::io::stdin().read_to_string(&mut raw)?;
    let credential: CredentialBundle =
        serde_json::from_str(&raw).map_err(|_| "stdin must contain credential JSON")?;
    if venue == "OKX" && credential.passphrase.as_deref().is_none_or(str::is_empty) {
        return Err("OKX credentials require passphrase".into());
    }
    Ok(credential)
}

fn canonical_venue(value: &str) -> Result<&'static str, Box<dyn Error>> {
    match value.to_ascii_uppercase().as_str() {
        "BYBIT" => Ok("BYBIT"),
        "BINANCE" => Ok("BINANCE"),
        "OKX" => Ok("OKX"),
        _ => Err("venue must be BYBIT, BINANCE, or OKX".into()),
    }
}

fn canonical_product(value: &str) -> Result<&'static str, Box<dyn Error>> {
    match value.to_ascii_uppercase().as_str() {
        "SPOT" => Ok("SPOT"),
        "LINEAR_PERPETUAL" | "PERP" => Ok("LINEAR_PERPETUAL"),
        _ => Err("product must be SPOT or LINEAR_PERPETUAL".into()),
    }
}

fn canonical_environment(venue: &str, value: &str) -> Result<&'static str, Box<dyn Error>> {
    match value.to_ascii_uppercase().as_str() {
        "PRODUCTION" => Ok("PRODUCTION"),
        "TESTNET" if venue != "OKX" => Ok("TESTNET"),
        "DEMO" if venue == "OKX" => Ok("DEMO"),
        _ => Err("environment is incompatible with venue".into()),
    }
}

fn mask_key(value: &str) -> String {
    let suffix = value.chars().rev().take(4).collect::<String>();
    format!("****{}", suffix.chars().rev().collect::<String>())
}

const fn usage() -> &'static str {
    "usage: venue-admin import VENUE ALIAS PRODUCT ENVIRONMENT | rotate SECRET_REF ACCOUNT_ID VENUE PRODUCT | disable ACCOUNT_ID | inspect"
}
