//! Paper scanner supervisor (M2 slice 1).
//!
//! Refuses any non-paper mode, then runs the deterministic funding-arbitrage
//! scanner on an interval, persisting each ranked snapshot for the API to
//! serve. No venue connectivity, credentials, or order placement exist.

use std::env;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use chrono::Utc;
use funding_arb_domain::TradingMode;
use funding_arb_scanner_service::{Scanner, ScannerRepo};
use sqlx::postgres::PgPoolOptions;

const DEFAULT_INTERVAL_SECS: u64 = 30;

fn main() -> ExitCode {
    let mode = env::var("PERPETO_MODE").unwrap_or_else(|_| String::from("PAPER"));
    if TradingMode::from_str(&mode) != Ok(TradingMode::Paper) {
        eprintln!("M2 engine refuses every mode except PAPER");
        return ExitCode::FAILURE;
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run())
}

async fn run() -> ExitCode {
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
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("failed to connect to the database: {error}");
            return ExitCode::FAILURE;
        }
    };

    let interval_secs = env::var("PERPETO_SCAN_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS);
    let scanner = Scanner::default();
    let repo = ScannerRepo::new(pool);

    println!("Perpeto M2 engine started; paper scanner active (interval {interval_secs}s)");
    loop {
        match scanner.scan(Utc::now()) {
            Ok(result) => {
                let count = result.opportunities.len();
                if let Err(error) = repo.record_scan(&result).await {
                    eprintln!("failed to persist scan: {error}");
                } else {
                    println!("scan complete: {count} eligible opportunities");
                }
            }
            Err(error) => eprintln!("scan failed: {error}"),
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}
