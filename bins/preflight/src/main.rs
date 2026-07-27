//! Scheduled credential preflight worker (M4 Slice 6).
//!
//! Keeps `ELIGIBLE` honest: on a fixed interval it re-runs the same preflight
//! composition the API endpoint uses ([`funding_arb_venue_service::PreflightService`])
//! against every connection currently marked `ELIGIBLE`, and fail-closed demotes
//! any that no longer pass their config checks or live reconciliation to
//! `PREFLIGHT_FAILED`. Promoting `PENDING_PREFLIGHT`/`PREFLIGHT_FAILED` stays
//! operator-triggered through the endpoint; this worker only demotes on drift.
//!
//! Like the reconciler, it requires SANDBOX or SHADOW mode and refuses to run in
//! PAPER (where no secret backend or connector exists) or LIVE.

use std::{env, process::ExitCode, str::FromStr, time::Duration};

use funding_arb_domain::TradingMode;
use funding_arb_tenant_context::{TenantContext, TenantId, assert_safe_runtime_role};
use funding_arb_venue_service::{PreflightService, VenueAccountsRepo};
use sqlx::postgres::PgPoolOptions;

const STATUS_ELIGIBLE: &str = "ELIGIBLE";

#[tokio::main]
async fn main() -> ExitCode {
    let mode = env::var("PERPETO_MODE").unwrap_or_else(|_| "PAPER".into());
    let mode = match TradingMode::from_str(&mode) {
        Ok(mode @ (TradingMode::Sandbox | TradingMode::Shadow)) => mode,
        _ => {
            eprintln!("preflight worker requires SANDBOX or SHADOW mode");
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
        eprintln!("unsafe preflight database role: {error}");
        return ExitCode::FAILURE;
    }
    let interval = env::var("PERPETO_PREFLIGHT_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(300);
    let service = match PreflightService::from_env(pool.clone()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let accounts = VenueAccountsRepo::new(pool.clone());
    println!("Perpeto credential preflight worker started in {mode} mode");
    loop {
        let tenants = match active_tenants(&pool).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("failed to load active tenants: {error}");
                return ExitCode::FAILURE;
            }
        };
        for tenant in tenants {
            let venue_accounts = match accounts.list(tenant).await {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("failed to list venue accounts: {error}");
                    continue;
                }
            };
            for account in venue_accounts {
                // Only re-validate connections that are currently ELIGIBLE; the
                // worker's job is demote-on-drift, not promotion.
                let eligible: Vec<String> = account
                    .connections
                    .iter()
                    .filter(|connection| connection.status == STATUS_ELIGIBLE)
                    .map(|connection| connection.product.clone())
                    .collect();
                for product in eligible {
                    match service.run(tenant, mode, &account, &product).await {
                        Ok(outcome) => {
                            if outcome.status != STATUS_ELIGIBLE {
                                println!(
                                    "preflight demoted {} {} {}: {} [{}]",
                                    account.venue,
                                    account.alias,
                                    product,
                                    outcome.status,
                                    outcome.checks.join(","),
                                );
                            }
                        }
                        Err(error) => eprintln!(
                            "preflight failed for {} {} {}: {error}",
                            account.venue, account.alias, product,
                        ),
                    }
                }
            }
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
