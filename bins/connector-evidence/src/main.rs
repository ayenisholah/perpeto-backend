use std::{env, error::Error, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use funding_arb_connectors::{
    AccountId, BinanceConnector, BybitConnector, MarketEvent, MarketSubscription, OkxConnector,
    PrivateEvent, ProductType, TimeRange, VenueConnector, VenueEnvironment,
    fixtures::ContractSuite,
};
use funding_arb_domain::{TradingMode, market::Venue};
use funding_arb_tenant_context::{TenantContext, TenantId};
use funding_arb_venue_service::{
    ConfiguredConnector, ConnectorRegistry, FeedStatus, PublicMarketConfig, PublicMarketSource,
    ReconciliationService,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    match env::args().nth(1).as_deref() {
        Some("fixtures") => fixture_evidence(),
        Some("public") => public_evidence().await,
        Some("snapshot") => snapshot_evidence().await,
        Some("credentialed") => credentialed_evidence().await,
        _ => Err(usage().into()),
    }
}

fn fixture_evidence() -> Result<(), Box<dyn Error>> {
    let evidence = [Venue::Bybit, Venue::Binance, Venue::Okx]
        .into_iter()
        .map(|venue| {
            let suite = ContractSuite::load(venue)?;
            Ok(json!({
                "venue": venue.as_str(),
                "status": "PASS",
                "schema_version": suite.manifest.schema_version,
                "api_revision": suite.manifest.api_revision,
                "reviewed_at": suite.manifest.reviewed_at,
                "cases_sha256": suite.manifest.cases_sha256,
                "case_count": suite.cases.len(),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    println!("{}", serde_json::to_string_pretty(&evidence)?);
    Ok(())
}

async fn public_evidence() -> Result<(), Box<dyn Error>> {
    let connectors: [Arc<dyn VenueConnector>; 3] = [
        Arc::new(BybitConnector::new(VenueEnvironment::Production, None)),
        Arc::new(BinanceConnector::new(VenueEnvironment::Production, None)),
        Arc::new(OkxConnector::new(VenueEnvironment::Production, None)),
    ];
    let mut evidence = Vec::new();
    for connector in connectors {
        evidence.push(probe_public(connector.as_ref()).await?);
    }
    println!("{}", serde_json::to_string_pretty(&evidence)?);
    Ok(())
}

/// One live-data PAPER snapshot (DEC-0016) from the keyless public source, as
/// the engine would take it. Reports per-venue feed health and leg counts.
async fn snapshot_evidence() -> Result<(), Box<dyn Error>> {
    let config = PublicMarketConfig::default()
        .with_underlyings(env::var("PERPETO_PAPER_UNDERLYINGS").ok().as_deref())?;
    let snapshot = PublicMarketSource::production(config)
        .snapshot(Utc::now())
        .await;
    let venues: Vec<Value> = snapshot
        .venues
        .iter()
        .map(|feed| {
            let legs = snapshot
                .legs
                .iter()
                .filter(|leg| leg.observation.instrument.venue == feed.venue)
                .count();
            json!({
                "venue": feed.venue.as_str(),
                "status": match feed.status {
                    FeedStatus::Healthy => "HEALTHY",
                    FeedStatus::Degraded => "DEGRADED",
                    FeedStatus::Unavailable => "UNAVAILABLE",
                },
                "legs": legs,
                "observed_at": feed.observed_at.map(|value| value.to_rfc3339()),
                "error": feed.error,
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "taken_at": snapshot.taken_at.to_rfc3339(),
            "legs": snapshot.legs.len(),
            "venues": venues,
        }))?
    );
    Ok(())
}

async fn credentialed_evidence() -> Result<(), Box<dyn Error>> {
    let mode = parse_mode()?;
    let database_url = env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&database_url)
        .await?;
    let registry = ConnectorRegistry::from_env(pool.clone())?;
    let tenant = quarantine_tenant()?;
    let configured = registry.load(tenant, mode).await?;
    if configured.is_empty() {
        return Err("no enabled connector accounts match the selected mode".into());
    }
    let selected_account = env::var("PERPETO_EVIDENCE_ACCOUNT_ID").ok();
    let mut completed = 0_u32;
    for connector in configured.into_iter().filter(|configured| {
        selected_account
            .as_deref()
            .is_none_or(|account_id| configured.account.id == account_id)
    }) {
        run_credentialed_probe(&pool, &connector).await?;
        completed += 1;
    }
    if completed == 0 {
        return Err("PERPETO_EVIDENCE_ACCOUNT_ID did not match an enabled account".into());
    }
    println!("recorded {completed} sanitized credentialed connector probe(s)");
    Ok(())
}

async fn probe_public(connector: &dyn VenueConnector) -> Result<Value, Box<dyn Error>> {
    let health = connector.health().await?;
    let server_time = connector.server_time().await?;
    let instruments = connector.instruments().await?;
    let instrument = instruments
        .iter()
        .find(|instrument| {
            instrument.id.product == ProductType::LinearPerpetual
                && instrument.canonical_underlying == "BTC"
        })
        .ok_or("BTC linear perpetual is unavailable")?;
    let funding = connector.current_funding(&instrument.id).await?;
    let mut stream = connector
        .market_stream(vec![MarketSubscription {
            instrument: instrument.id.clone(),
            depth: 50,
        }])
        .await?;
    let event = tokio::time::timeout(Duration::from_secs(20), stream.next())
        .await
        .map_err(|_| "market stream timed out")?
        .ok_or("market stream ended")??;
    Ok(json!({
        "venue": connector.venue().as_str(),
        "status": "PASS",
        "environment": health.environment.as_str(),
        "health": format!("{:?}", health.status).to_ascii_uppercase(),
        "clock_drift_ms": health.clock_drift_ms,
        "server_time": server_time.venue_time.to_rfc3339(),
        "instrument_count": instruments.len(),
        "btc_funding_observed_at": funding.observed_at.to_rfc3339(),
        "market_event": market_event_kind(&event),
    }))
}

async fn run_credentialed_probe(
    pool: &PgPool,
    configured: &ConfiguredConnector,
) -> Result<(), Box<dyn Error>> {
    let started_at = Utc::now();
    let account_id = AccountId(configured.account.id.clone());
    let public = probe_public(configured.connector.as_ref()).await;
    let private = async {
        let balances = configured.connector.balances(&account_id).await?;
        let positions = configured.connector.positions(&account_id).await?;
        let orders = configured.connector.open_orders(&account_id).await?;
        let fills = configured.connector.fills(&account_id, None).await?;
        let fees = configured.connector.fee_schedule(&account_id).await?;
        let now = Utc::now();
        let funding = configured
            .connector
            .funding_ledger(
                &account_id,
                &TimeRange {
                    start: now - chrono::Duration::days(2),
                    end: now,
                },
            )
            .await?;
        let mut private_stream = configured.connector.private_stream(&account_id).await?;
        let private_event = tokio::time::timeout(Duration::from_secs(20), private_stream.next())
            .await
            .map_err(|_| funding_arb_connectors::ConnectorError {
                kind: funding_arb_connectors::ConnectorErrorKind::Transient,
                code: "PRIVATE_STREAM_TIMEOUT".into(),
                message: "private stream did not produce a fresh event".into(),
                retry_after_ms: None,
            })?
            .ok_or_else(|| funding_arb_connectors::ConnectorError {
                kind: funding_arb_connectors::ConnectorErrorKind::Transient,
                code: "PRIVATE_STREAM_ENDED".into(),
                message: "private stream ended before producing an event".into(),
                retry_after_ms: None,
            })??;
        Ok::<Value, funding_arb_connectors::ConnectorError>(json!({
            "balance_count": balances.len(),
            "position_count": positions.len(),
            "open_order_count": orders.len(),
            "fill_count": fills.fills.len(),
            "fill_has_more": fills.next_cursor.is_some(),
            "fee_schedule_count": fees.len(),
            "funding_ledger_count": funding.len(),
            "private_stream_event": private_event_kind(&private_event),
        }))
    }
    .await;
    let reconciliation = ReconciliationService::new(pool.clone())
        .run(
            configured.tenant,
            configured.connector.as_ref(),
            &account_id,
        )
        .await;
    let (status, payload) = match (public, private, reconciliation) {
        (Ok(public), Ok(private), Ok(reconciliation)) => (
            "PASS",
            json!({
                "public": public,
                "private": private,
                "reconciliation": {
                    "status": reconciliation.status,
                    "discrepancies": reconciliation.discrepancies,
                },
                "writes_attempted": false,
            }),
        ),
        (public, private, reconciliation) => (
            "FAIL",
            json!({
                "public": public.err().map(|error| error.to_string()),
                "private": private.err().map(|error| error.to_string()),
                "reconciliation": reconciliation.err().map(|error| error.to_string()),
                "writes_attempted": false,
            }),
        ),
    };
    persist_evidence(pool, configured, status, started_at, Utc::now(), &payload).await?;
    if status == "FAIL" {
        return Err(format!(
            "{} {} credentialed probe failed",
            configured.account.venue,
            configured.product.as_str()
        )
        .into());
    }
    Ok(())
}

fn quarantine_tenant() -> Result<TenantContext, uuid::Error> {
    Uuid::parse_str("00000000-0000-0000-0000-000000000001")
        .map(TenantId::new)
        .map(TenantContext::new)
}

async fn persist_evidence(
    pool: &PgPool,
    configured: &ConfiguredConnector,
    status: &str,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    payload: &Value,
) -> Result<(), Box<dyn Error>> {
    sqlx::query(
        "INSERT INTO connector_certification_evidence
         (id, venue, venue_account_id, product, environment, probe_kind, status,
          started_at, completed_at, payload)
         VALUES ($1, $2, $3, $4, $5, 'CREDENTIALLED_READ_RECONCILE', $6, $7, $8, $9)",
    )
    .bind(Uuid::new_v4())
    .bind(&configured.account.venue)
    .bind(Uuid::parse_str(&configured.account.id)?)
    .bind(configured.product.as_str())
    .bind(configured.environment.as_str())
    .bind(status)
    .bind(started_at)
    .bind(completed_at)
    .bind(serde_json::to_string(payload)?)
    .execute(pool)
    .await?;
    Ok(())
}

fn parse_mode() -> Result<TradingMode, Box<dyn Error>> {
    match env::var("PERPETO_MODE").as_deref() {
        Ok("SANDBOX") => Ok(TradingMode::Sandbox),
        Ok("SHADOW") => Ok(TradingMode::Shadow),
        _ => Err("credentialed evidence requires PERPETO_MODE=SANDBOX or SHADOW".into()),
    }
}

const fn market_event_kind(event: &MarketEvent) -> &'static str {
    match event {
        MarketEvent::BookSnapshot { .. } => "BOOK_SNAPSHOT",
        MarketEvent::BookDelta { .. } => "BOOK_DELTA",
        MarketEvent::Funding(_) => "FUNDING",
        MarketEvent::InstrumentChanged(_) => "INSTRUMENT_CHANGED",
        MarketEvent::Heartbeat(_) => "HEARTBEAT",
    }
}

const fn private_event_kind(event: &PrivateEvent) -> &'static str {
    match event {
        PrivateEvent::Order(_) => "ORDER",
        PrivateEvent::Fill(_) => "FILL",
        PrivateEvent::Balance(_) => "BALANCE",
        PrivateEvent::Position(_) => "POSITION",
        PrivateEvent::Funding(_) => "FUNDING",
        PrivateEvent::Heartbeat(_) => "HEARTBEAT",
    }
}

const fn usage() -> &'static str {
    "usage: funding-arb-connector-evidence fixtures | public | snapshot | credentialed"
}
