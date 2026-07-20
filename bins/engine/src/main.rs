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
use funding_arb_domain::position::ExitReason;
use funding_arb_execution_service::{
    BreakerAction, BreakerScope, BreakersRepo, ExecutionService, OutboxRepo, PositionDto,
    PositionsRepo,
};
use funding_arb_scanner_service::fixtures::{self, MarketStatus};
use funding_arb_scanner_service::{Scanner, ScannerRepo};
use rust_decimal::Decimal;
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
    let positions = PositionsRepo::new(pool.clone());
    let breakers = BreakersRepo::new(pool.clone());
    let outbox = OutboxRepo::new(pool);
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
        activate_automatic_breakers(&positions, &breakers, &outbox, &execution, now).await;
        supervise_open_positions(&positions, &breakers, &execution, now).await;
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
    breakers: &BreakersRepo,
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
    let controls = match breakers.controls().await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to load circuit breakers: {error}");
            return;
        }
    };
    if controls.effective_action == Some(BreakerAction::ReadOnly) {
        return;
    }
    for dto in stored
        .into_iter()
        .filter(|position| position.state == "OPEN")
    {
        let id = dto.id.clone();
        let flatten = controls
            .breakers
            .iter()
            .any(|breaker| breaker.action == BreakerAction::Flatten);
        let disabled = controls.breakers.iter().find(|breaker| {
            breaker.action == BreakerAction::DisableVenue
                && breaker.scope_key.as_ref().is_some_and(|venue| {
                    dto.long_leg.venue == *venue || dto.short_leg.venue == *venue
                })
        });
        let result = if flatten {
            Ok(execution.close_with_reason(dto.to_domain(), now, ExitReason::CircuitBreaker))
        } else if let Some(breaker) = disabled {
            let reason = if breaker.source == "VENUE_HEALTH" {
                ExitReason::VenueUnhealthy
            } else {
                ExitReason::CircuitBreaker
            };
            Ok(execution.close_with_reason(dto.to_domain(), now, reason))
        } else {
            execution.supervise(dto.to_domain(), now)
        };
        match result {
            Ok(position) => {
                if let Err(error) = positions.update(&PositionDto::from_domain(&position)).await {
                    eprintln!("failed to persist supervised position {id}: {error}");
                }
            }
            Err(error) => eprintln!("failed to supervise position {id}: {error}"),
        }
    }
}

async fn activate_automatic_breakers(
    positions: &PositionsRepo,
    breakers: &BreakersRepo,
    outbox: &OutboxRepo,
    execution: &ExecutionService,
    now: DateTime<Utc>,
) {
    if let Ok(legs) = fixtures::load_at(now) {
        for leg in legs
            .iter()
            .filter(|leg| leg.status == MarketStatus::Degraded)
        {
            ensure_breaker(
                breakers,
                outbox,
                BreakerScope::Venue,
                Some(leg.observation.instrument.venue.as_str().to_owned()),
                BreakerAction::DisableVenue,
                "VENUE_HEALTH",
                "Fixture venue health is degraded",
                now,
            )
            .await;
        }
    }
    let stored = match positions.list().await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to evaluate loss breakers: {error}");
            return;
        }
    };
    let mut closed = stored
        .iter()
        .filter(|position| position.state == "CLOSED")
        .map(PositionDto::to_domain)
        .collect::<Vec<_>>();
    closed.sort_by_key(|position| position.closed_at);
    let reference = execution.scanner.risk_limits.reference_equity;
    let daily_pnl: Decimal = closed
        .iter()
        .filter(|position| {
            position
                .closed_at
                .is_some_and(|at| at.date_naive() == now.date_naive())
        })
        .map(|position| position.realized_pnl)
        .sum();
    if daily_pnl < -(reference * execution.scanner.risk_limits.max_daily_loss_fraction) {
        ensure_breaker(
            breakers,
            outbox,
            BreakerScope::Global,
            None,
            BreakerAction::HaltNewRisk,
            "DAILY_LOSS",
            "Daily realized loss limit breached",
            now,
        )
        .await;
    }
    let mut equity = reference;
    let mut peak = reference;
    for position in closed {
        equity += position.realized_pnl;
        peak = peak.max(equity);
    }
    let drawdown = if peak.is_zero() {
        Decimal::ZERO
    } else {
        (peak - equity) / peak
    };
    if drawdown >= execution.scanner.risk_limits.max_drawdown_fraction {
        ensure_breaker(
            breakers,
            outbox,
            BreakerScope::Global,
            None,
            BreakerAction::Flatten,
            "DRAWDOWN",
            "Portfolio drawdown limit breached",
            now,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn ensure_breaker(
    breakers: &BreakersRepo,
    outbox: &OutboxRepo,
    scope: BreakerScope,
    scope_key: Option<String>,
    action: BreakerAction,
    source: &str,
    reason: &str,
    now: DateTime<Utc>,
) {
    let existing = match breakers.controls().await {
        Ok(value) => value.breakers.into_iter().any(|breaker| {
            breaker.scope == scope && breaker.scope_key == scope_key && breaker.action == action
        }),
        Err(error) => {
            eprintln!("failed to inspect automatic breakers: {error}");
            return;
        }
    };
    if existing {
        return;
    }
    match breakers
        .activate(
            scope,
            scope_key,
            action,
            source,
            reason.to_owned(),
            None,
            now,
        )
        .await
    {
        Ok(breaker) => {
            let payload = serde_json::to_value(&breaker).unwrap_or_default();
            if let Err(error) = outbox
                .append("CIRCUIT_BREAKER_ACTIVATED", &breaker.id, payload, now)
                .await
            {
                eprintln!("failed to append breaker event: {error}");
            }
        }
        Err(error) => eprintln!("failed to activate automatic breaker: {error}"),
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
