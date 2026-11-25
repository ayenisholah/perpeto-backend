use std::{collections::HashSet, env, process::ExitCode, str::FromStr, time::Duration};

use chrono::Utc;
use funding_arb_domain::TradingMode;
use funding_arb_tenant_context::{TenantContext, TenantId, assert_safe_runtime_role};
use funding_arb_venue_service::{
    ConnectorExecutionCoordinator, ConnectorRegistry, ReconciliationService, ReconciliationState,
    VenueAccountsRepo,
};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> ExitCode {
    let mode = env::var("PERPETO_MODE").unwrap_or_else(|_| "PAPER".into());
    let mode = match TradingMode::from_str(&mode) {
        Ok(TradingMode::Sandbox) => TradingMode::Sandbox,
        Ok(TradingMode::Shadow) => TradingMode::Shadow,
        _ => {
            eprintln!("connector reconciler requires SANDBOX or SHADOW mode");
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
    // Reject an unsafe role before building the registry or announcing startup,
    // matching the api and engine binaries: the registry resolves credential
    // backend configuration, which must not run under a role that can bypass
    // tenant isolation.
    if let Err(error) = assert_safe_runtime_role(&pool).await {
        eprintln!("unsafe reconciler database role: {error}");
        return ExitCode::FAILURE;
    }
    let interval = env::var("PERPETO_RECONCILE_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(60);
    let registry = match ConnectorRegistry::from_env(pool.clone()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    // `reconciler once` runs a single pass and exits, for operator jobs such as
    // the M4 sandbox runs; with no argument it loops on the interval.
    let once = env::args().nth(1).as_deref() == Some("once");
    let service = ReconciliationService::new(pool.clone());
    let coordinator = ConnectorExecutionCoordinator::new(pool.clone());
    let accounts = VenueAccountsRepo::new(pool.clone());
    println!("Perpeto connector reconciler started in {mode} mode");
    loop {
        let tenants = match active_tenants(&pool).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("failed to load active tenants: {error}");
                return ExitCode::FAILURE;
            }
        };
        for tenant in tenants {
            let configured = match registry.load(tenant, mode).await {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("failed to load connector registry: {error}");
                    continue;
                }
            };
            // Query before retry: resolve every ambiguous submission against
            // the venue before the account is reconciled. Until resolved, the
            // coordinator refuses to resend that client order id.
            match coordinator.unresolved(tenant).await {
                Ok(unresolved) => {
                    for intent in unresolved {
                        let Some(owner) = configured
                            .iter()
                            .find(|candidate| candidate.account.id == intent.venue_account_id)
                        else {
                            eprintln!(
                                "unknown outcome {} has no configured account",
                                intent.intent_id
                            );
                            continue;
                        };
                        let account_id =
                            funding_arb_connectors::AccountId(intent.venue_account_id.clone());
                        match coordinator
                            .reconcile_unknown(
                                tenant,
                                owner.connector.as_ref(),
                                &account_id,
                                &intent.client_order_id,
                            )
                            .await
                        {
                            Ok(Some(order)) => println!(
                                "resolved unknown outcome {} ({}): venue status {:?}",
                                intent.intent_id, intent.client_order_id, order.status,
                            ),
                            Ok(None) => println!(
                                "resolved unknown outcome {} ({}): not found at the venue",
                                intent.intent_id, intent.client_order_id,
                            ),
                            Err(error) => eprintln!(
                                "unknown outcome {} still unresolved: {error}",
                                intent.intent_id
                            ),
                        }
                    }
                }
                Err(error) => eprintln!("failed to list unknown outcomes: {error}"),
            }
            let mut seen = HashSet::new();
            for configured in configured {
                if !seen.insert(configured.account.id.clone()) {
                    continue;
                }
                let account_id = funding_arb_connectors::AccountId(configured.account.id.clone());
                match service
                    .run(tenant, configured.connector.as_ref(), &account_id)
                    .await
                {
                    Ok(report) => {
                        println!(
                            "reconciliation {} {}: {} ({} discrepancies)",
                            configured.account.venue,
                            configured.account.alias,
                            report.status,
                            report.discrepancies,
                        );
                        let mut account = configured.account;
                        account.reconciliation_state = if report.discrepancies == 0 {
                            ReconciliationState::Reconciled
                        } else {
                            ReconciliationState::Required
                        };
                        account.last_reconciled_at = Some(Utc::now().to_rfc3339());
                        if let Err(error) = accounts.update(tenant, &account).await {
                            eprintln!("failed to update reconciliation state: {error}");
                        }
                    }
                    Err(error) => eprintln!(
                        "reconciliation failed for {} {}: {error}",
                        configured.account.venue, configured.account.alias,
                    ),
                }
            }
        }
        if once {
            return ExitCode::SUCCESS;
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

async fn active_tenants(pool: &sqlx::PgPool) -> Result<Vec<TenantContext>, sqlx::Error> {
    sqlx::query_scalar::<_, uuid::Uuid>(
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
