//! Credential preflight compose (M4 Slice 6).
//!
//! A connection reaches `ELIGIBLE` only when its declared configuration is sound
//! *and* a live reconciliation against the venue succeeds. This module owns that
//! composition so it runs identically whether an operator triggers it through the
//! API (`bins/api` `preflight_credential`) or the scheduled worker
//! (`bins/preflight`) re-validates it. Fail-closed: no `ELIGIBLE` without both
//! halves passing, and any drift on a re-run demotes to `PREFLIGHT_FAILED`.

use std::sync::Arc;

use funding_arb_connectors::{AccountId, ProductType};
use funding_arb_domain::TradingMode;
use funding_arb_tenant_context::TenantContext;

use crate::dto::{ProductConnectionDto, VenueAccountDto};
use crate::enrollment::{CredentialEnrollmentService, EnrollmentError};
use crate::reconcile::ReconciliationService;
use crate::registry::{ConnectorRegistry, RegistryError};

/// The reconciliation half of a preflight, summarised for reporting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreflightReconciliationSummary {
    pub status: String,
    pub balances_seen: usize,
    pub positions_seen: usize,
    pub open_orders_seen: usize,
    pub discrepancies: usize,
}

/// The outcome of a preflight pass over a single connection.
#[derive(Clone, Debug)]
pub struct PreflightOutcome {
    /// Resulting connection status: `ELIGIBLE` or `PREFLIGHT_FAILED`.
    pub status: String,
    /// Human-readable checks, e.g. `SECRET_PRESENT`, `RECONCILE:RECONCILED`.
    pub checks: Vec<String>,
    /// Present only when the config half passed and reconciliation was attempted.
    pub reconciliation: Option<PreflightReconciliationSummary>,
}

/// A redacted preflight failure. Reconciliation *failure* is not an error here —
/// it simply yields `PREFLIGHT_FAILED`; only structural problems surface.
#[derive(Debug)]
pub enum PreflightError {
    /// No connection for the requested product on the account.
    NotFound,
    /// The connection's product string is not a recognised connector product.
    InvalidProduct,
    /// Loading the connector registry failed.
    Registry(RegistryError),
    /// Recording the preflight outcome failed.
    Enrollment(EnrollmentError),
}

impl std::fmt::Display for PreflightError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("venue account connection not found"),
            Self::InvalidProduct => formatter.write_str("invalid connector product"),
            Self::Registry(error) => write!(formatter, "{error}"),
            Self::Enrollment(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PreflightError {}

/// Evaluate the connection's declared configuration. Pure and DB-free: returns
/// the ordered check labels and whether every gate passed. `ELIGIBLE` requires a
/// stored secret, no withdrawal permission, read access, and an environment
/// permitted by the current runtime mode.
#[must_use]
pub fn evaluate_config(
    connection: &ProductConnectionDto,
    mode: TradingMode,
) -> (Vec<String>, bool) {
    let has_secret = connection.secret_ref.is_some();
    let no_withdrawal = connection.withdrawal_permission != Some(true);
    let read_permission = connection.read_permission;
    let environment_valid = match mode {
        TradingMode::Sandbox => connection.environment != "PRODUCTION",
        TradingMode::Shadow => {
            connection.environment == "PRODUCTION" && !connection.trade_permission
        }
        TradingMode::Paper => true,
        TradingMode::Live => false,
    };
    let checks = vec![
        if has_secret {
            "SECRET_PRESENT"
        } else {
            "SECRET_MISSING"
        }
        .to_owned(),
        if no_withdrawal {
            "NO_WITHDRAWAL"
        } else {
            "WITHDRAWAL_ENABLED"
        }
        .to_owned(),
        if read_permission {
            "READ_PERMISSION"
        } else {
            "NO_READ_PERMISSION"
        }
        .to_owned(),
        if environment_valid {
            "ENVIRONMENT_VALID"
        } else {
            "ENVIRONMENT_INVALID"
        }
        .to_owned(),
    ];
    let config_ok = has_secret && no_withdrawal && read_permission && environment_valid;
    (checks, config_ok)
}

fn parse_product(product: &str) -> Result<ProductType, PreflightError> {
    match product {
        "SPOT" => Ok(ProductType::Spot),
        "LINEAR_PERPETUAL" => Ok(ProductType::LinearPerpetual),
        _ => Err(PreflightError::InvalidProduct),
    }
}

/// Composes the connector registry, live reconciliation, and the credential
/// state machine into a single preflight pass. Shared by the API endpoint and
/// the scheduled worker.
pub struct PreflightService {
    registry: Arc<ConnectorRegistry>,
    reconciliation: ReconciliationService,
    enrollment: Arc<CredentialEnrollmentService>,
}

impl PreflightService {
    #[must_use]
    pub fn new(
        registry: Arc<ConnectorRegistry>,
        reconciliation: ReconciliationService,
        enrollment: Arc<CredentialEnrollmentService>,
    ) -> Self {
        Self {
            registry,
            reconciliation,
            enrollment,
        }
    }

    /// Resolve all collaborators from the environment (Vault or AES backend),
    /// mirroring [`ConnectorRegistry::from_env`]. Used by the standalone worker.
    ///
    /// # Errors
    /// Returns [`RegistryError`] when no credential backend is configured.
    pub fn from_env(pool: sqlx::PgPool) -> Result<Self, RegistryError> {
        let registry = Arc::new(ConnectorRegistry::from_env(pool.clone())?);
        let reconciliation = ReconciliationService::new(pool.clone());
        let enrollment = Arc::new(CredentialEnrollmentService::from_env(pool)?);
        Ok(Self::new(registry, reconciliation, enrollment))
    }

    /// Run a full preflight over one connection and record the result. Returns
    /// the resulting status plus the checks that determined it. Fail-closed: any
    /// config or reconciliation failure records `PREFLIGHT_FAILED`.
    ///
    /// # Errors
    /// Returns [`PreflightError`] when the connection is missing, its product is
    /// unrecognised, the registry cannot load, or persistence fails.
    pub async fn run(
        &self,
        tenant: TenantContext,
        mode: TradingMode,
        account: &VenueAccountDto,
        product: &str,
    ) -> Result<PreflightOutcome, PreflightError> {
        let connection = account
            .connections
            .iter()
            .find(|connection| connection.product == product)
            .ok_or(PreflightError::NotFound)?;
        let (mut checks, config_ok) = evaluate_config(connection, mode);

        // Live reconciliation only runs when the declared config is sound.
        let mut reconciliation = None;
        let mut reconcile_ok = false;
        if config_ok {
            let product_type = parse_product(product)?;
            let configured = self
                .registry
                .load(tenant, mode)
                .await
                .map_err(PreflightError::Registry)?
                .into_iter()
                .find(|configured| {
                    configured.account.id == account.id && configured.product == product_type
                });
            match configured {
                Some(configured) => {
                    match self
                        .reconciliation
                        .run(
                            tenant,
                            configured.connector.as_ref(),
                            &AccountId(account.id.clone()),
                        )
                        .await
                    {
                        Ok(report) => {
                            reconcile_ok =
                                report.status == "RECONCILED" && report.discrepancies == 0;
                            checks.push(format!("RECONCILE:{}", report.status));
                            reconciliation = Some(PreflightReconciliationSummary {
                                status: report.status,
                                balances_seen: report.balances_seen,
                                positions_seen: report.positions_seen,
                                open_orders_seen: report.open_orders_seen,
                                discrepancies: report.discrepancies,
                            });
                        }
                        Err(_) => checks.push("RECONCILE:FAILED".to_owned()),
                    }
                }
                None => checks.push("CONNECTOR_UNAVAILABLE".to_owned()),
            }
        }

        let updated = self
            .enrollment
            .record_preflight_result(tenant, &account.id, product, config_ok && reconcile_ok)
            .await
            .map_err(PreflightError::Enrollment)?;
        let status = updated
            .connections
            .into_iter()
            .find(|connection| connection.product == product)
            .map(|connection| connection.status)
            .unwrap_or_default();
        Ok(PreflightOutcome {
            status,
            checks,
            reconciliation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(environment: &str, withdrawal: Option<bool>, read: bool) -> ProductConnectionDto {
        ProductConnectionDto {
            id: "c".to_owned(),
            product: "LINEAR_PERPETUAL".to_owned(),
            environment: environment.to_owned(),
            secret_ref: Some("s".to_owned()),
            read_permission: read,
            trade_permission: false,
            withdrawal_permission: withdrawal,
            status: "ELIGIBLE".to_owned(),
            last_tested_at: None,
        }
    }

    #[test]
    fn sound_testnet_config_passes_in_sandbox() {
        let (checks, ok) = evaluate_config(
            &connection("TESTNET", Some(false), true),
            TradingMode::Sandbox,
        );
        assert!(ok);
        assert!(checks.contains(&"SECRET_PRESENT".to_owned()));
        assert!(checks.contains(&"NO_WITHDRAWAL".to_owned()));
        assert!(checks.contains(&"ENVIRONMENT_VALID".to_owned()));
    }

    #[test]
    fn missing_secret_fails_config() {
        let mut connection = connection("TESTNET", Some(false), true);
        connection.secret_ref = None;
        let (checks, ok) = evaluate_config(&connection, TradingMode::Sandbox);
        assert!(!ok);
        assert!(checks.contains(&"SECRET_MISSING".to_owned()));
    }

    #[test]
    fn enabled_withdrawal_fails_config() {
        let (checks, ok) = evaluate_config(
            &connection("TESTNET", Some(true), true),
            TradingMode::Sandbox,
        );
        assert!(!ok);
        assert!(checks.contains(&"WITHDRAWAL_ENABLED".to_owned()));
    }

    #[test]
    fn missing_read_permission_fails_config() {
        let (checks, ok) = evaluate_config(
            &connection("TESTNET", Some(false), false),
            TradingMode::Sandbox,
        );
        assert!(!ok);
        assert!(checks.contains(&"NO_READ_PERMISSION".to_owned()));
    }

    #[test]
    fn production_environment_is_invalid_in_sandbox() {
        let (checks, ok) = evaluate_config(
            &connection("PRODUCTION", Some(false), true),
            TradingMode::Sandbox,
        );
        assert!(!ok);
        assert!(checks.contains(&"ENVIRONMENT_INVALID".to_owned()));
    }

    #[test]
    fn shadow_requires_production_without_trade_permission() {
        let (_, ok) = evaluate_config(
            &connection("PRODUCTION", Some(false), true),
            TradingMode::Shadow,
        );
        assert!(ok);
        let mut trades = connection("PRODUCTION", Some(false), true);
        trades.trade_permission = true;
        let (_, ok) = evaluate_config(&trades, TradingMode::Shadow);
        assert!(!ok);
    }

    #[test]
    fn live_mode_never_validates() {
        let (_, ok) = evaluate_config(&connection("TESTNET", Some(false), true), TradingMode::Live);
        assert!(!ok);
    }

    #[test]
    fn unknown_product_is_rejected() {
        assert!(matches!(
            parse_product("OPTION"),
            Err(PreflightError::InvalidProduct)
        ));
        assert!(parse_product("SPOT").is_ok());
        assert!(parse_product("LINEAR_PERPETUAL").is_ok());
    }
}
