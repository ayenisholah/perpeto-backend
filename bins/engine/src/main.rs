//! Paper scanner supervisor (M2 slice 1).
//!
//! Refuses any non-paper mode, then runs the deterministic funding-arbitrage
//! scanner on an interval, persisting each ranked snapshot for the API to
//! serve. No venue connectivity, credentials, or order placement exist.

use std::env;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use funding_arb_domain::TradingMode;
use funding_arb_execution_service::{ExecutionService, PositionDto, PositionsRepo};
use funding_arb_scanner_service::{Scanner, ScannerRepo};
use sqlx::postgres::PgPoolOptions;

const DEFAULT_INTERVAL_SECS: u64 = 30;
const DEFAULT_SIM_TICK_SECS: u64 = 14_400;
const DEFAULT_SIM_START: &str = "2026-07-19T00:00:00Z";

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
    let sim_tick_secs = match positive_seconds(
        "PERPETO_SIM_TICK_SECS",
        env::var("PERPETO_SIM_TICK_SECS").ok().as_deref(),
        DEFAULT_SIM_TICK_SECS,
    ) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let sim_start = match parse_sim_start(env::var("PERPETO_SIM_START").ok().as_deref()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let scanner = Scanner::default();
    let scan_repo = ScannerRepo::new(pool.clone());
    let positions = PositionsRepo::new(pool);
    let execution = ExecutionService {
        scanner,
        ..ExecutionService::default()
    };

    println!(
        "Perpeto M2 engine started; paper scanner and position supervision active \
         (sleep {interval_secs}s, simulation step {sim_tick_secs}s)",
    );
    let mut tick = 0_u64;
    loop {
        let Some(now) = simulated_now(sim_start, tick, sim_tick_secs) else {
            eprintln!("paper simulation clock overflowed");
            return ExitCode::FAILURE;
        };
        supervise_open_positions(&positions, &execution, now).await;
        match scanner.scan(now) {
            Ok(result) => {
                let count = result.opportunities.len();
                if let Err(error) = scan_repo.record_scan(&result).await {
                    eprintln!("failed to persist scan: {error}");
                } else {
                    println!("scan complete at {now}: {count} eligible opportunities");
                }
            }
            Err(error) => eprintln!("scan failed: {error}"),
        }
        tick = match tick.checked_add(1) {
            Some(value) => value,
            None => {
                eprintln!("paper simulation tick overflowed");
                return ExitCode::FAILURE;
            }
        };
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}

async fn supervise_open_positions(
    positions: &PositionsRepo,
    execution: &ExecutionService,
    now: DateTime<Utc>,
) {
    let stored = match positions.list().await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to load positions for supervision: {error}");
            return;
        }
    };
    for dto in stored
        .into_iter()
        .filter(|position| position.state == "OPEN")
    {
        let id = dto.id.clone();
        match execution.supervise(dto.to_domain(), now) {
            Ok(position) => {
                if let Err(error) = positions.update(&PositionDto::from_domain(&position)).await {
                    eprintln!("failed to persist supervised position {id}: {error}");
                }
            }
            Err(error) => eprintln!("failed to supervise position {id}: {error}"),
        }
    }
}

fn positive_seconds(name: &str, value: Option<&str>, default: u64) -> Result<u64, String> {
    let parsed = match value {
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| format!("{name} must be a positive integer"))?,
        None => default,
    };
    if parsed == 0 {
        Err(format!("{name} must be greater than zero"))
    } else {
        Ok(parsed)
    }
}

fn parse_sim_start(value: Option<&str>) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value.unwrap_or(DEFAULT_SIM_START))
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| "PERPETO_SIM_START must be an RFC 3339 timestamp".to_owned())
}

fn simulated_now(start: DateTime<Utc>, tick: u64, step_secs: u64) -> Option<DateTime<Utc>> {
    let elapsed = tick.checked_mul(step_secs)?;
    let elapsed = i64::try_from(elapsed).ok()?;
    start.checked_add_signed(chrono::Duration::seconds(elapsed))
}

#[cfg(test)]
mod tests {
    use super::{parse_sim_start, positive_seconds, simulated_now};
    use chrono::{TimeZone, Utc};

    #[test]
    fn simulation_defaults_to_t0_and_advances_four_hours_per_tick() {
        let start = parse_sim_start(None).expect("default start");
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap(),);
        assert_eq!(
            simulated_now(start, 3, 14_400),
            Some(Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap()),
        );
    }

    #[test]
    fn invalid_clock_configuration_is_rejected() {
        assert!(parse_sim_start(Some("not-a-time")).is_err());
        assert!(positive_seconds("STEP", Some("0"), 1).is_err());
        assert!(positive_seconds("STEP", Some("nope"), 1).is_err());
        assert_eq!(positive_seconds("STEP", None, 7), Ok(7));
    }
}
