use std::{collections::HashSet, env, process::ExitCode, str::FromStr, time::Duration};

use chrono::Utc;
use funding_arb_domain::TradingMode;
use funding_arb_venue_service::{
    ConnectorRegistry, ReconciliationService, ReconciliationState, VenueAccountsRepo,
    decode_master_key,
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
    let master_key = match env::var("PERPETO_SECRET_MASTER_KEY")
        .map_err(|_| "PERPETO_SECRET_MASTER_KEY is required".to_owned())
        .and_then(|value| decode_master_key(&value).map_err(|error| error.to_string()))
    {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
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
    let interval = env::var("PERPETO_RECONCILE_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(60);
    let registry = ConnectorRegistry::new(pool.clone(), master_key);
    let service = ReconciliationService::new(pool.clone());
    let accounts = VenueAccountsRepo::new(pool);
    println!("Perpeto connector reconciler started in {mode} mode");
    loop {
        let configured = match registry.load(mode).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("failed to load connector registry: {error}");
                tokio::time::sleep(Duration::from_secs(interval)).await;
                continue;
            }
        };
        let mut seen = HashSet::new();
        for configured in configured {
            if !seen.insert(configured.account.id.clone()) {
                continue;
            }
            let account_id = funding_arb_connectors::AccountId(configured.account.id.clone());
            match service
                .run(configured.connector.as_ref(), &account_id)
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
                    if let Err(error) = accounts.update(&account).await {
                        eprintln!("failed to update reconciliation state: {error}");
                    }
                }
                Err(error) => eprintln!(
                    "reconciliation failed for {} {}: {error}",
                    configured.account.venue, configured.account.alias,
                ),
            }
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}
