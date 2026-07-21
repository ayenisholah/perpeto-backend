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
use funding_arb_connectors::{MarketEvent, MarketSubscription, ProductType};
use funding_arb_domain::TradingMode;
use funding_arb_domain::market::{FundingObservation, InstrumentRef};
use funding_arb_domain::position::ExitReason;
use funding_arb_execution_service::{
    BreakerAction, BreakerScope, BreakersRepo, ExecutionService, OutboxRepo, PositionDto,
    PositionsRepo,
};
use funding_arb_scanner_service::fixtures::{self, InstrumentKind, MarketLeg, MarketStatus};
use funding_arb_scanner_service::{Scanner, ScannerRepo};
use funding_arb_strategy_service::{PositionSummary, StrategiesRepo, StrategyService};
use funding_arb_tenant_context::{TenantContext, TenantId, assert_safe_runtime_role};
use funding_arb_venue_service::{
    ConfiguredConnector, ConnectorRegistry, InstrumentDto, VenueAccountsRepo, decode_master_key,
};
use futures_util::StreamExt;
use rust_decimal::Decimal;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;

const DEFAULT_INTERVAL_SECS: u64 = 30;
const DEFAULT_SIM_TICK_SECS: u64 = 14_400;
const DEFAULT_SIM_START: &str = "2026-07-19T00:00:00Z";

fn main() -> ExitCode {
    let mode = env::var("PERPETO_MODE").unwrap_or_else(|_| String::from("PAPER"));
    let mode = match TradingMode::from_str(&mode) {
        Ok(mode) if !mode.is_live() => mode,
        Ok(_) => {
            eprintln!("LIVE mode remains locked until certification");
            return ExitCode::FAILURE;
        }
        Err(_) => {
            eprintln!("invalid PERPETO_MODE");
            return ExitCode::FAILURE;
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to start async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(run(mode))
}

async fn run(mode: TradingMode) -> ExitCode {
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
    if let Err(error) = assert_safe_runtime_role(&pool).await {
        eprintln!("unsafe runtime database role: {error}");
        return ExitCode::FAILURE;
    }

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
    let outbox = OutboxRepo::new(pool.clone());
    let strategies = StrategiesRepo::new(pool.clone());
    let venues = VenueAccountsRepo::new(pool.clone());
    let configured = if mode.is_paper() {
        Vec::new()
    } else {
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
        let registry = ConnectorRegistry::new(pool.clone(), master_key);
        let mut value = Vec::new();
        let tenants = match active_tenant_contexts(&pool).await {
            Ok(tenants) => tenants,
            Err(error) => {
                eprintln!("failed to load active tenants: {error}");
                return ExitCode::FAILURE;
            }
        };
        for tenant in tenants {
            match registry.load(tenant, mode).await {
                Ok(mut configured) => value.append(&mut configured),
                Err(error) => {
                    eprintln!("failed to load connector registry: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        if value.is_empty() {
            eprintln!("no enabled connector accounts match {mode} mode");
            return ExitCode::FAILURE;
        }
        value
    };
    let execution = ExecutionService {
        scanner,
        ..ExecutionService::default()
    };

    println!(
        "Perpeto engine started in {mode} mode (sleep {interval_secs}s, simulation step {sim_tick_secs}s)",
    );
    let mut tick = 0_u64;
    loop {
        let now = if mode.is_paper() {
            let Some(now) = simulated_now(sim_start, tick, sim_tick_secs) else {
                eprintln!("paper simulation clock overflowed");
                return ExitCode::FAILURE;
            };
            now
        } else {
            Utc::now()
        };
        let tenant_contexts = match active_tenant_contexts(&pool).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("failed to dispatch tenant work: {error}");
                return ExitCode::FAILURE;
            }
        };
        if mode.is_paper() {
            for tenant in &tenant_contexts {
                activate_automatic_breakers(
                    *tenant, &positions, &breakers, &outbox, &execution, now,
                )
                .await;
                supervise_open_positions(*tenant, &positions, &breakers, &execution, now).await;
            }
        }
        let scan = if mode.is_paper() {
            scanner.scan(now).map_err(|error| error.to_string())
        } else {
            live_scan(&scanner, &configured, &venues, now).await
        };
        match scan {
            Ok(result) => {
                let count = result.opportunities.len();
                if let Err(error) = scan_repo.record_scan(&result).await {
                    eprintln!("failed to persist scan: {error}");
                } else {
                    println!("scan complete at {now}: {count} eligible opportunities");
                    if mode.is_paper() {
                        for tenant in &tenant_contexts {
                            auto_enter(
                                *tenant,
                                &result.opportunities,
                                &strategies,
                                &positions,
                                &breakers,
                                &execution,
                                now,
                            )
                            .await;
                        }
                    }
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

async fn live_scan(
    scanner: &Scanner,
    configured: &[ConfiguredConnector],
    venues: &VenueAccountsRepo,
    now: DateTime<Utc>,
) -> Result<funding_arb_scanner_service::ScanResult, String> {
    let mut legs = Vec::new();
    for configured in configured {
        let health = configured
            .connector
            .health()
            .await
            .map_err(|error| format!("{} health: {error}", configured.account.venue))?;
        venues
            .record_health(configured.tenant, &configured.account.id, &health)
            .await
            .map_err(|_| "failed to persist connector health".to_owned())?;
        let status = match health.status {
            funding_arb_connectors::HealthStatus::Healthy => MarketStatus::Healthy,
            _ => MarketStatus::Degraded,
        };
        let instruments = configured
            .connector
            .instruments()
            .await
            .map_err(|error| format!("{} instruments: {error}", configured.account.venue))?;
        for instrument in instruments
            .into_iter()
            .filter(|instrument| instrument.id.product == configured.product)
        {
            venues
                .upsert_instrument(&instrument_dto(&instrument))
                .await
                .map_err(|_| "failed to persist exact instrument metadata".to_owned())?;
            let subscription = MarketSubscription {
                instrument: instrument.id.clone(),
                depth: 50,
            };
            let mut stream = configured
                .connector
                .market_stream(vec![subscription])
                .await
                .map_err(|error| format!("{} market stream: {error}", configured.account.venue))?;
            let event = stream
                .next()
                .await
                .ok_or_else(|| "connector market stream ended".to_owned())?
                .map_err(|error| format!("connector market snapshot: {error}"))?;
            let (book_mark, available_notional, observed_at) =
                book_measurement(event, instrument.contract_multiplier)?;
            let (funding_rate, interval_hours, mark_price, index_price, next_funding_at) =
                if configured.product == ProductType::LinearPerpetual {
                    let funding = configured
                        .connector
                        .current_funding(&instrument.id)
                        .await
                        .map_err(|error| {
                            format!("{} funding: {error}", configured.account.venue)
                        })?;
                    (
                        funding.rate,
                        funding.interval_hours,
                        funding.mark_price,
                        funding.index_price,
                        funding.next_funding_at,
                    )
                } else {
                    (
                        Decimal::ZERO,
                        8,
                        book_mark,
                        book_mark,
                        now + chrono::Duration::hours(8),
                    )
                };
            legs.push(MarketLeg {
                kind: if configured.product == ProductType::Spot {
                    InstrumentKind::Spot
                } else {
                    InstrumentKind::Perp
                },
                status,
                observation: FundingObservation {
                    instrument: InstrumentRef {
                        venue: instrument.id.venue,
                        symbol: instrument.id.symbol,
                        underlying: instrument.canonical_underlying,
                    },
                    funding_rate,
                    interval_hours,
                    mark_price,
                    index_price,
                    available_notional,
                    observed_at,
                    next_funding_at,
                },
            });
        }
    }
    Ok(scanner.scan_legs(now, &legs))
}

fn book_measurement(
    event: MarketEvent,
    contract_multiplier: Decimal,
) -> Result<(Decimal, Decimal, DateTime<Utc>), String> {
    let MarketEvent::BookSnapshot {
        bids,
        asks,
        received_at,
        ..
    } = event
    else {
        return Err("connector did not provide a book snapshot".into());
    };
    let best_bid = bids
        .first()
        .ok_or_else(|| "connector book has no bids".to_owned())?;
    let best_ask = asks
        .first()
        .ok_or_else(|| "connector book has no asks".to_owned())?;
    if best_bid.price >= best_ask.price {
        return Err("connector book is crossed".into());
    }
    let mark = (best_bid.price + best_ask.price) / Decimal::new(2, 0);
    let bid_notional: Decimal = bids
        .iter()
        .map(|level| level.price * level.quantity * contract_multiplier)
        .sum();
    let ask_notional: Decimal = asks
        .iter()
        .map(|level| level.price * level.quantity * contract_multiplier)
        .sum();
    Ok((mark, bid_notional.min(ask_notional), received_at))
}

fn instrument_dto(instrument: &funding_arb_connectors::Instrument) -> InstrumentDto {
    InstrumentDto {
        venue: instrument.id.venue.as_str().into(),
        symbol: instrument.id.symbol.clone(),
        product: instrument.id.product.as_str().into(),
        canonical_underlying: instrument.canonical_underlying.clone(),
        quote_asset: instrument.quote_asset.clone(),
        settlement_asset: instrument.settlement_asset.clone(),
        margin_asset: instrument.margin_asset.clone(),
        contract_multiplier: instrument.contract_multiplier.to_string(),
        price_tick: instrument.price_tick.to_string(),
        quantity_step: instrument.quantity_step.to_string(),
        minimum_quantity: instrument.minimum_quantity.to_string(),
        maximum_quantity: instrument.maximum_quantity.map(|value| value.to_string()),
        minimum_notional: instrument.minimum_notional.to_string(),
        funding_interval_hours: instrument.funding_interval_hours,
        funding_cap: instrument.funding_cap.map(|value| value.to_string()),
        funding_floor: instrument.funding_floor.map(|value| value.to_string()),
        maximum_leverage: instrument.maximum_leverage.map(|value| value.to_string()),
        trading: instrument.trading,
        metadata_version: instrument.metadata_version.clone(),
        venue_updated_at: instrument.venue_updated_at.map(|value| value.to_rfc3339()),
        received_at: instrument.received_at.to_rfc3339(),
    }
}

async fn auto_enter(
    tenant: TenantContext,
    opportunities: &[funding_arb_domain::opportunity::Opportunity],
    strategies: &StrategiesRepo,
    positions: &PositionsRepo,
    breakers: &BreakersRepo,
    execution: &ExecutionService,
    now: DateTime<Utc>,
) {
    let controls = match breakers.controls(tenant).await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to read controls for auto-entry: {error}");
            return;
        }
    };
    if controls.effective_action.is_some() {
        return;
    }
    let stored = match positions.list(tenant).await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to read positions for auto-entry: {error}");
            return;
        }
    };
    let summaries = stored
        .iter()
        .map(|position| {
            let domain = position.to_domain();
            PositionSummary {
                strategy_id: position.strategy_id.clone(),
                state: domain.state,
                at: domain.closed_at.unwrap_or(domain.opened_at),
                realized_pnl: domain.realized_pnl,
            }
        })
        .collect::<Vec<_>>();
    let allocated: Decimal = stored
        .iter()
        .filter(|position| position.state != "CLOSED")
        .map(|position| position.reserved_capital)
        .sum();
    let free_equity =
        (execution.scanner.risk_limits.reference_equity - allocated).max(Decimal::ZERO);
    let configured = match strategies.list(tenant).await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to read strategies for auto-entry: {error}");
            return;
        }
    };
    for dto in configured.into_iter().filter(|strategy| strategy.enabled) {
        let Some(strategy) = dto.to_domain() else {
            continue;
        };
        for intent in StrategyService::evaluate(
            &strategy,
            opportunities,
            &summaries,
            &execution.scanner.risk_limits,
            free_equity,
            now,
        ) {
            let key = format!(
                "strategy:{}:{}:{}",
                intent.strategy_id, strategy.current.version, intent.route_id
            );
            if positions
                .get_by_idempotency_key(tenant, &key)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                continue;
            }
            match execution.open(&intent.route_id, intent.notional, now) {
                Ok(mut position) => {
                    position.strategy_id = Some(intent.strategy_id);
                    let dto = PositionDto::from_domain(&position);
                    if let Err(error) = positions.create(tenant, &dto, &key).await {
                        eprintln!("failed to persist auto-entry: {error}");
                    }
                }
                Err(error) => eprintln!("strategy auto-entry failed: {error}"),
            }
        }
    }
}

async fn supervise_open_positions(
    tenant: TenantContext,
    positions: &PositionsRepo,
    breakers: &BreakersRepo,
    execution: &ExecutionService,
    now: DateTime<Utc>,
) {
    let stored = match positions.list(tenant).await {
        Ok(value) => value,
        Err(error) => {
            eprintln!("failed to load positions for supervision: {error}");
            return;
        }
    };
    let controls = match breakers.controls(tenant).await {
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
                if let Err(error) = positions
                    .update(tenant, &PositionDto::from_domain(&position))
                    .await
                {
                    eprintln!("failed to persist supervised position {id}: {error}");
                }
            }
            Err(error) => eprintln!("failed to supervise position {id}: {error}"),
        }
    }
}

async fn activate_automatic_breakers(
    tenant: TenantContext,
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
                tenant,
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
    let stored = match positions.list(tenant).await {
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
            tenant,
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
            tenant,
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
    tenant: TenantContext,
    breakers: &BreakersRepo,
    outbox: &OutboxRepo,
    scope: BreakerScope,
    scope_key: Option<String>,
    action: BreakerAction,
    source: &str,
    reason: &str,
    now: DateTime<Utc>,
) {
    let existing = match breakers.controls(tenant).await {
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
            tenant,
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
                .append(
                    tenant,
                    "CIRCUIT_BREAKER_ACTIVATED",
                    &breaker.id,
                    payload,
                    now,
                )
                .await
            {
                eprintln!("failed to append breaker event: {error}");
            }
        }
        Err(error) => eprintln!("failed to activate automatic breaker: {error}"),
    }
}

async fn active_tenant_contexts(pool: &sqlx::PgPool) -> Result<Vec<TenantContext>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id FROM tenants WHERE state NOT IN ('SUSPENDED', 'DELETED') ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| TenantContext::new(TenantId::new(row.get("id"))))
        .collect())
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
