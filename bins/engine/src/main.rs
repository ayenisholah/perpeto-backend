//! The scan, supervision and paper-entry loop.
//!
//! PAPER reads a market snapshot every tick, either keyless live public data
//! from Binance, Bybit and OKX (`PERPETO_PAPER_SOURCE=live`, the default,
//! DEC-0016) or the deterministic fixtures on simulated time
//! (`PERPETO_PAPER_SOURCE=fixtures`). It supervises open paper positions,
//! ranks the snapshot, persists the scan with the snapshot, and lets enabled
//! strategies enter, all against that one snapshot. PAPER never loads
//! credentials. SANDBOX and SHADOW scan configured connector accounts; LIVE
//! is refused.

use std::env;
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use funding_arb_domain::TradingMode;
use funding_arb_domain::position::{ExitReason, PaperFillScenario};
use funding_arb_execution_service::{
    BreakerAction, BreakerScope, BreakersRepo, ExecutionService, OutboxRepo, PositionDto,
    PositionsRepo, fixture_snapshot,
};
use funding_arb_scanner_service::fixtures::MarketStatus;
use funding_arb_scanner_service::{FeedStatus, MarketSnapshot, Scanner, ScannerRepo};
use funding_arb_strategy_service::{PositionSummary, StrategiesRepo, StrategyService};
use funding_arb_tenant_context::{TenantContext, TenantId, assert_safe_runtime_role};
use funding_arb_venue_service::{
    ConfiguredConnector, ConnectorRegistry, DEFAULT_DEPTH, DEFAULT_VENUE_TIMEOUT, InstrumentDto,
    PublicMarketConfig, PublicMarketSource, VenueAccountsRepo, measure_leg,
};
use rust_decimal::Decimal;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;

const DEFAULT_INTERVAL_SECS: u64 = 30;
const DEFAULT_SIM_TICK_SECS: u64 = 14_400;
const DEFAULT_SIM_START: &str = "2026-07-19T00:00:00Z";

/// Where PAPER reads its market data (DEC-0016).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PaperSource {
    /// Keyless live public data from the venues, on wall-clock time.
    Live,
    /// The embedded deterministic fixtures, on simulated time.
    Fixtures,
}

/// The market state one engine tick acts on.
#[derive(Clone, Copy)]
struct Tick<'a> {
    snapshot: &'a MarketSnapshot,
    now: DateTime<Utc>,
}

/// Parse `PERPETO_PAPER_SOURCE`; unset means `live`.
fn parse_paper_source(value: Option<&str>) -> Result<PaperSource, String> {
    match value.map(|raw| raw.trim().to_ascii_lowercase()).as_deref() {
        None | Some("live") => Ok(PaperSource::Live),
        Some("fixtures") => Ok(PaperSource::Fixtures),
        Some(other) => Err(format!(
            "PERPETO_PAPER_SOURCE must be `live` or `fixtures`, got {other:?}"
        )),
    }
}

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
    let paper_source = match parse_paper_source(env::var("PERPETO_PAPER_SOURCE").ok().as_deref()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let public_source = if mode.is_paper() && paper_source == PaperSource::Live {
        match PublicMarketConfig::default()
            .with_underlyings(env::var("PERPETO_PAPER_UNDERLYINGS").ok().as_deref())
        {
            Ok(config) => Some(PublicMarketSource::production(config)),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
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
        let registry = match ConnectorRegistry::from_env(pool.clone()) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        };
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

    match (mode.is_paper(), &public_source) {
        (true, Some(source)) => println!(
            "Perpeto engine started in {mode} mode on live public data for {} (sleep {interval_secs}s)",
            source.config().underlyings.join(","),
        ),
        (true, None) => println!(
            "Perpeto engine started in {mode} mode on fixtures (sleep {interval_secs}s, simulation step {sim_tick_secs}s)",
        ),
        (false, _) => println!("Perpeto engine started in {mode} mode (sleep {interval_secs}s)"),
    }
    let mut tick = 0_u64;
    loop {
        let now = if mode.is_paper() && public_source.is_none() {
            let Some(now) = simulated_now(sim_start, tick, sim_tick_secs) else {
                eprintln!("paper simulation clock overflowed");
                return ExitCode::FAILURE;
            };
            now
        } else {
            Utc::now()
        };
        // PAPER works from one snapshot per tick; SANDBOX/SHADOW scan accounts.
        let snapshot = if mode.is_paper() {
            match paper_snapshot(public_source.as_ref(), now).await {
                Ok(snapshot) => Some(snapshot),
                Err(error) => {
                    eprintln!("scan failed: {error}");
                    None
                }
            }
        } else {
            None
        };
        let tenant_contexts = match active_tenant_contexts(&pool).await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("failed to dispatch tenant work: {error}");
                return ExitCode::FAILURE;
            }
        };
        if let Some(snapshot) = &snapshot {
            for tenant in &tenant_contexts {
                activate_automatic_breakers(
                    *tenant, &positions, &breakers, &outbox, &execution, snapshot, now,
                )
                .await;
                supervise_open_positions(*tenant, &positions, &breakers, &execution, snapshot, now)
                    .await;
            }
        }
        let scan = match &snapshot {
            Some(snapshot) => Some(Ok(scanner.scan_legs(now, &snapshot.legs))),
            None if mode.is_paper() => None,
            None => Some(live_scan(&scanner, &configured, &venues, now).await),
        };
        match scan {
            None => {}
            Some(Ok(result)) => {
                let count = result.opportunities.len();
                if let Err(error) = scan_repo
                    .record_scan_with_snapshot(&result, snapshot.as_ref())
                    .await
                {
                    eprintln!("failed to persist scan: {error}");
                } else {
                    println!("scan complete at {now}: {count} eligible opportunities");
                    if let Some(snapshot) = &snapshot {
                        report_feeds(snapshot);
                        // Strategy evaluation consumes bare opportunities. The
                        // scanner already ranks behind its risk gates, so
                        // re-checking `within_limits` can only narrow the set,
                        // and entries must fail closed.
                        let entry_candidates: Vec<_> = result
                            .opportunities
                            .iter()
                            .filter(|ranked| ranked.assessment.within_limits)
                            .map(|ranked| ranked.opportunity.clone())
                            .collect();
                        for tenant in &tenant_contexts {
                            auto_enter(
                                *tenant,
                                &entry_candidates,
                                &strategies,
                                &positions,
                                &breakers,
                                &execution,
                                Tick { snapshot, now },
                            )
                            .await;
                        }
                    }
                }
            }
            Some(Err(error)) => eprintln!("scan failed: {error}"),
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

/// One PAPER snapshot: live public data, or the fixtures at `now`.
async fn paper_snapshot(
    public_source: Option<&PublicMarketSource>,
    now: DateTime<Utc>,
) -> Result<MarketSnapshot, String> {
    match public_source {
        Some(source) => Ok(source.snapshot(now).await),
        None => fixture_snapshot(now).map_err(|error| error.to_string()),
    }
}

/// Log every venue whose public feed is not healthy, so a silent venue is
/// visible in the engine log as well as in the stored snapshot.
fn report_feeds(snapshot: &MarketSnapshot) {
    for feed in snapshot
        .venues
        .iter()
        .filter(|feed| feed.status != FeedStatus::Healthy)
    {
        eprintln!(
            "venue feed {} is {}: {}",
            feed.venue,
            feed.status.as_str(),
            feed.error
                .as_deref()
                .unwrap_or("venue reported degraded health"),
        );
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
            // Shared with live-data PAPER (DEC-0016) so the credentialed and
            // public paths measure a leg identically.
            let (leg, _book) = measure_leg(
                configured.connector.as_ref(),
                &instrument,
                status,
                DEFAULT_DEPTH,
                DEFAULT_VENUE_TIMEOUT,
            )
            .await?;
            legs.push(leg);
        }
    }
    Ok(scanner.scan_legs(now, &legs))
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
    tick: Tick<'_>,
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
            tick.now,
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
            match execution.open_from(
                tick.snapshot,
                &intent.route_id,
                Some(intent.notional),
                key.clone(),
                PaperFillScenario::default(),
                tick.now,
            ) {
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
    snapshot: &MarketSnapshot,
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
            execution.supervise_with(snapshot, dto.to_domain(), now)
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
    snapshot: &MarketSnapshot,
    now: DateTime<Utc>,
) {
    {
        for leg in snapshot
            .legs
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
                "Venue feed health is degraded",
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
    use super::{
        PaperSource, paper_snapshot, parse_paper_source, parse_sim_start, positive_seconds,
        simulated_now,
    };
    use chrono::{TimeZone, Utc};

    #[test]
    fn paper_source_defaults_to_live_and_rejects_unknown_values() {
        assert_eq!(parse_paper_source(None), Ok(PaperSource::Live));
        assert_eq!(parse_paper_source(Some(" LIVE ")), Ok(PaperSource::Live));
        assert_eq!(
            parse_paper_source(Some("fixtures")),
            Ok(PaperSource::Fixtures)
        );
        assert!(parse_paper_source(Some("replay")).is_err());
        assert!(parse_paper_source(Some("")).is_err());
    }

    #[tokio::test]
    async fn fixture_mode_snapshots_carry_the_fixture_legs_and_no_books() {
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 0, 0, 0).unwrap();
        let snapshot = paper_snapshot(None, now).await.expect("fixture snapshot");
        assert_eq!(snapshot.taken_at, now);
        assert!(!snapshot.legs.is_empty());
        assert!(snapshot.books.is_empty());
        assert!(snapshot.venues.is_empty());
    }

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
