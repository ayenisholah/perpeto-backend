//! Builds connector instances from masked account records and encrypted secrets.

use std::{env, error::Error, fmt, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use funding_arb_connectors::{
    BinanceConnector, BybitConnector, OkxConnector, ProductType, VenueConnector, VenueEnvironment,
};
use funding_arb_domain::TradingMode;
use funding_arb_tenant_context::TenantContext;
use sqlx::PgPool;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    EncryptedPostgresSecretStore, SecretContext, SecretStore, VaultConfig,
    VaultTransitSecretStore, VenueAccountDto, VenueAccountsRepo,
};

#[derive(Clone)]
pub struct ConfiguredConnector {
    pub tenant: TenantContext,
    pub account: VenueAccountDto,
    pub product: ProductType,
    pub environment: VenueEnvironment,
    pub connector: Arc<dyn VenueConnector>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError {
    InvalidMasterKey,
    InvalidConfiguration,
    Persistence,
    SecretUnavailable,
    ModeForbidden,
    MissingBackend,
    VaultMisconfigured,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMasterKey => "invalid connector secret master key",
            Self::InvalidConfiguration => "invalid venue account configuration",
            Self::Persistence => "venue account persistence failed",
            Self::SecretUnavailable => "venue account secret is unavailable",
            Self::ModeForbidden => "runtime mode forbids configured connector environment",
            Self::MissingBackend => {
                "no credential backend configured; set PERPETO_VAULT_ADDR or PERPETO_SECRET_MASTER_KEY"
            }
            Self::VaultMisconfigured => {
                "vault backend selected but its configuration is incomplete"
            }
        })
    }
}

impl Error for RegistryError {}

pub struct ConnectorRegistry {
    accounts: VenueAccountsRepo,
    secrets: Box<dyn SecretStore>,
}

impl ConnectorRegistry {
    /// Build a registry backed by the transitional AES master-key store. Used
    /// by paper/dev and tests; live deployments should use
    /// [`Self::with_secret_store`] with the Vault backend.
    #[must_use]
    pub fn new(pool: PgPool, master_key: [u8; 32]) -> Self {
        let accounts = VenueAccountsRepo::new(pool.clone());
        Self {
            accounts,
            secrets: Box::new(EncryptedPostgresSecretStore::new(pool, master_key, 1)),
        }
    }

    /// Build a registry over any [`SecretStore`] backend (AES or Vault Transit).
    #[must_use]
    pub fn with_secret_store(pool: PgPool, secrets: Box<dyn SecretStore>) -> Self {
        Self {
            accounts: VenueAccountsRepo::new(pool),
            secrets,
        }
    }

    /// Resolve the credential backend from the environment: Vault Transit when
    /// `PERPETO_VAULT_ADDR` is set, otherwise the transitional AES master key.
    /// Outside PAPER exactly one must resolve, else the process should refuse to
    /// start.
    ///
    /// # Errors
    /// Returns [`RegistryError`] when Vault is misconfigured or neither backend
    /// is available.
    pub fn from_env(pool: PgPool) -> Result<Self, RegistryError> {
        let secrets = secret_store_from_env(pool.clone())?;
        Ok(Self::with_secret_store(pool, secrets))
    }

    pub async fn load(
        &self,
        context: TenantContext,
        mode: TradingMode,
    ) -> Result<Vec<ConfiguredConnector>, RegistryError> {
        if mode.is_paper() {
            return Ok(Vec::new());
        }
        if mode.is_live() {
            return Err(RegistryError::ModeForbidden);
        }
        let accounts = self
            .accounts
            .list(context)
            .await
            .map_err(|_| RegistryError::Persistence)?;
        let mut connectors = Vec::new();
        for account in accounts.into_iter().filter(|account| account.enabled) {
            let account_id =
                Uuid::parse_str(&account.id).map_err(|_| RegistryError::InvalidConfiguration)?;
            for connection in account
                .connections
                .iter()
                .filter(|connection| connection.status != "DISABLED")
            {
                let environment = parse_environment(&connection.environment)?;
                let environment_allowed = match mode {
                    TradingMode::Sandbox => environment.is_non_production(),
                    TradingMode::Shadow => environment == VenueEnvironment::Production,
                    TradingMode::Paper | TradingMode::Live => false,
                };
                if !environment_allowed {
                    return Err(RegistryError::ModeForbidden);
                }
                let product = parse_product(&connection.product)?;
                let secret_ref = connection
                    .secret_ref
                    .as_deref()
                    .ok_or(RegistryError::SecretUnavailable)
                    .and_then(|value| {
                        Uuid::parse_str(value).map_err(|_| RegistryError::InvalidConfiguration)
                    })?;
                let context = SecretContext {
                    tenant: context,
                    venue_account_id: account_id,
                    venue: account.venue.clone(),
                    product: connection.product.clone(),
                };
                let credential = self
                    .secrets
                    .load(secret_ref, &context)
                    .await
                    .map_err(|_| RegistryError::SecretUnavailable)?;
                let connector: Arc<dyn VenueConnector> = match account.venue.as_str() {
                    "BYBIT" => Arc::new(BybitConnector::new(
                        environment,
                        Some(credential.connector_credentials()),
                    )),
                    "BINANCE" => Arc::new(BinanceConnector::new(
                        environment,
                        Some(credential.connector_credentials()),
                    )),
                    "OKX" => Arc::new(OkxConnector::new(
                        environment,
                        Some(credential.connector_credentials()),
                    )),
                    _ => return Err(RegistryError::InvalidConfiguration),
                };
                connectors.push(ConfiguredConnector {
                    tenant: context,
                    account: account.clone(),
                    product,
                    environment,
                    connector,
                });
            }
        }
        Ok(connectors)
    }
}

/// Resolve the credential [`SecretStore`] from the environment: Vault Transit
/// when `PERPETO_VAULT_ADDR` is set, otherwise the transitional AES master-key
/// store. Shared by [`ConnectorRegistry::from_env`] and the `venue-admin` CLI.
///
/// # Errors
/// Returns [`RegistryError`] when Vault is misconfigured or neither backend is
/// available.
pub fn secret_store_from_env(pool: PgPool) -> Result<Box<dyn SecretStore>, RegistryError> {
    match VaultConfig::from_env() {
        Ok(Some(vault)) => Ok(Box::new(VaultTransitSecretStore::new(pool, vault))),
        Ok(None) => {
            let encoded =
                env::var("PERPETO_SECRET_MASTER_KEY").map_err(|_| RegistryError::MissingBackend)?;
            let master_key = decode_master_key(&encoded)?;
            Ok(Box::new(EncryptedPostgresSecretStore::new(pool, master_key, 1)))
        }
        Err(_) => Err(RegistryError::VaultMisconfigured),
    }
}

pub fn decode_master_key(encoded: &str) -> Result<[u8; 32], RegistryError> {
    let decoded = Zeroizing::new(
        STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| RegistryError::InvalidMasterKey)?,
    );
    decoded
        .as_slice()
        .try_into()
        .map_err(|_| RegistryError::InvalidMasterKey)
}

fn parse_product(value: &str) -> Result<ProductType, RegistryError> {
    match value {
        "SPOT" => Ok(ProductType::Spot),
        "LINEAR_PERPETUAL" => Ok(ProductType::LinearPerpetual),
        _ => Err(RegistryError::InvalidConfiguration),
    }
}

fn parse_environment(value: &str) -> Result<VenueEnvironment, RegistryError> {
    match value {
        "PRODUCTION" => Ok(VenueEnvironment::Production),
        "TESTNET" => Ok(VenueEnvironment::Testnet),
        "DEMO" => Ok(VenueEnvironment::Demo),
        _ => Err(RegistryError::InvalidConfiguration),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_key_requires_exactly_32_decoded_bytes() {
        assert!(decode_master_key("not-base64").is_err());
        assert!(decode_master_key(&STANDARD.encode([1_u8; 31])).is_err());
        assert_eq!(
            decode_master_key(&STANDARD.encode([1_u8; 32])).expect("valid key"),
            [1_u8; 32]
        );
    }
}
