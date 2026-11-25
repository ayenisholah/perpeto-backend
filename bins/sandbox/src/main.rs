//! `funding-arb-sandbox`: the M4 operator driver for testnet and demo venues.
//!
//! ```text
//! funding-arb-sandbox open    VENUE UNDERLYING NOTIONAL_USDT
//! funding-arb-sandbox close   VENUE
//! funding-arb-sandbox cycle   VENUE UNDERLYING NOTIONAL_USDT
//! funding-arb-sandbox recover VENUE UNDERLYING
//! funding-arb-sandbox report  VENUE SINCE_RFC3339 OUT_DIR
//! ```
//!
//! It requires `PERPETO_MODE=SANDBOX`, `DATABASE_URL` (the runtime role) and
//! `PERPETO_TENANT_ID` (the operator tenant from `venue-admin tenant-ensure`),
//! plus the credential store configuration the reconciler uses. It acts only on
//! the tenant's enrolled TESTNET or DEMO connections, and holds a per-venue
//! advisory lock so two drivers never trade one venue at once.

use std::{env, error::Error, fmt::Write as _, path::Path, str::FromStr, time::Duration};

use chrono::{DateTime, Utc};
use funding_arb_connectors::ProductType;
use funding_arb_domain::TradingMode;
use funding_arb_tenant_context::{
    TenantContext, TenantId, TenantTransaction, assert_safe_runtime_role,
};
use funding_arb_venue_service::{
    ConfiguredConnector, ConnectorRegistry, ReconciliationService, SandboxCycle, SandboxDriver,
    SandboxLeg, SandboxVenue,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use uuid::Uuid;

/// The cycle closes this long after the funding timestamp, so the settlement
/// has posted before the hedge is unwound.
const FUNDING_GRACE: Duration = Duration::from_secs(120);

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sandbox: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let command = arguments.first().ok_or(USAGE)?.as_str();
    let venue = arguments.get(1).ok_or(USAGE)?.to_ascii_uppercase();
    let mode = TradingMode::from_str(&env::var("PERPETO_MODE").unwrap_or_default())
        .map_err(|_| "PERPETO_MODE must be SANDBOX")?;
    if mode != TradingMode::Sandbox {
        return Err("PERPETO_MODE must be SANDBOX".into());
    }
    let tenant = TenantContext::new(TenantId::new(
        Uuid::parse_str(env::var("PERPETO_TENANT_ID")?.trim())
            .map_err(|_| "PERPETO_TENANT_ID must be a UUID")?,
    ));
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?)
        .await?;
    assert_safe_runtime_role(&pool).await?;
    let mut lock = pool.acquire().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext($1))")
        .bind(format!("perpeto-sandbox-{venue}"))
        .fetch_one(&mut *lock)
        .await?;
    if !locked {
        return Err(format!("another sandbox driver holds {venue}").into());
    }

    let configured = ConnectorRegistry::from_env(pool.clone())?
        .load(tenant, mode)
        .await?;
    let spot = leg(&configured, &venue, ProductType::Spot)?;
    let perp = leg(&configured, &venue, ProductType::LinearPerpetual)?;
    if spot.environment != perp.environment {
        return Err(format!("{venue} spot and perpetual connections differ in environment").into());
    }
    let legs = SandboxVenue {
        environment: perp.environment,
        spot: SandboxLeg {
            connector: spot.connector.as_ref(),
            account: account(spot),
        },
        perp: SandboxLeg {
            connector: perp.connector.as_ref(),
            account: account(perp),
        },
    };
    let driver = SandboxDriver::new(pool.clone(), mode)?;

    match command {
        "open" => {
            let cycle = driver
                .open(
                    tenant,
                    &legs,
                    underlying(&arguments)?,
                    notional(&arguments)?,
                )
                .await?;
            print_cycle("opened", &cycle);
        }
        "close" => print_cycle("closed", &driver.close(tenant, &legs).await?),
        "cycle" => {
            let cycle = match driver.active(tenant, &venue).await? {
                Some(cycle) if cycle.state == "OPEN" => {
                    println!("resuming open cycle {}", cycle.id);
                    cycle
                }
                Some(cycle) => {
                    println!("finishing interrupted cycle {} ({})", cycle.id, cycle.state);
                    print_cycle("closed", &driver.close(tenant, &legs).await?);
                    return Ok(());
                }
                None => {
                    let cycle = driver
                        .open(
                            tenant,
                            &legs,
                            underlying(&arguments)?,
                            notional(&arguments)?,
                        )
                        .await?;
                    print_cycle("opened", &cycle);
                    cycle
                }
            };
            let funding_at = cycle.funding_at.ok_or("the cycle has no funding time")?;
            wait_until(funding_at).await;
            print_cycle("closed", &driver.close(tenant, &legs).await?);
        }
        "recover" => {
            let outcome = driver
                .recover(tenant, &legs, underlying(&arguments)?)
                .await?;
            println!(
                "recovery {}: client order {} first {:?}, retry {:?}, resolved {:?}, final {:?}, filled {}",
                outcome.cycle_id,
                outcome.client_order_id,
                outcome.first,
                outcome.retry,
                outcome.resolved,
                outcome.final_status,
                outcome.filled,
            );
            if outcome.first != funding_arb_venue_service::SubmissionState::UnknownOutcome {
                println!(
                    "the fault did not trigger: rerun with a smaller PERPETO_FAULT_SUBMIT_TIMEOUT_MS"
                );
            }
        }
        "report" => {
            let since = DateTime::parse_from_rfc3339(arguments.get(2).ok_or(USAGE)?)
                .map_err(|_| "SINCE must be RFC 3339, e.g. 2026-11-26T00:00:00Z")?
                .with_timezone(&Utc);
            let out = Path::new(arguments.get(3).ok_or(USAGE)?);
            // Pull the venue's latest orders, fills and funding into the
            // database first, so the report reads one reconciled view.
            let reconciler = ReconciliationService::new(pool.clone());
            for configured_leg in [&legs.spot, &legs.perp] {
                let summary = reconciler
                    .run(tenant, configured_leg.connector, &configured_leg.account)
                    .await
                    .map_err(|error| format!("reconciliation failed: {error:?}"))?;
                println!(
                    "reconciled {}: {} ({} fills, {} funding entries, {} discrepancies)",
                    hash(&configured_leg.account.0),
                    summary.status,
                    summary.fills_seen,
                    summary.funding_entries_seen,
                    summary.discrepancies
                );
            }
            let runs = driver.runs_since(tenant, &venue, since).await?;
            let report = build_report(&pool, tenant, &venue, since, &runs, &legs).await?;
            std::fs::create_dir_all(out)?;
            std::fs::write(
                out.join(format!("{venue}.json")),
                serde_json::to_string_pretty(&report)?,
            )?;
            std::fs::write(out.join(format!("{venue}.md")), render(&report))?;
            println!("wrote {}/{venue}.md and .json", out.display());
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

const USAGE: &str = "usage: funding-arb-sandbox open VENUE UNDERLYING NOTIONAL | close VENUE | \
                     cycle VENUE UNDERLYING NOTIONAL | recover VENUE UNDERLYING | \
                     report VENUE SINCE OUT_DIR";

fn leg<'a>(
    configured: &'a [ConfiguredConnector],
    venue: &str,
    product: ProductType,
) -> Result<&'a ConfiguredConnector, String> {
    let mut matching = configured
        .iter()
        .filter(|item| item.account.venue == venue && item.product == product);
    let found = matching
        .next()
        .ok_or_else(|| format!("no enrolled {venue} {} connection", product.as_str()))?;
    if matching.next().is_some() {
        return Err(format!(
            "more than one enrolled {venue} {} connection; disable the extra one",
            product.as_str()
        ));
    }
    Ok(found)
}

fn account(configured: &ConfiguredConnector) -> funding_arb_connectors::AccountId {
    funding_arb_connectors::AccountId(configured.account.id.clone())
}

fn underlying(arguments: &[String]) -> Result<&str, &'static str> {
    arguments.get(2).map(String::as_str).ok_or(USAGE)
}

fn notional(arguments: &[String]) -> Result<Decimal, Box<dyn Error>> {
    let value = arguments.get(3).ok_or(USAGE)?.parse::<Decimal>()?;
    if value <= Decimal::ZERO {
        return Err("NOTIONAL must be positive".into());
    }
    Ok(value)
}

async fn wait_until(funding_at: DateTime<Utc>) {
    let close_at = funding_at + chrono::Duration::from_std(FUNDING_GRACE).unwrap_or_default();
    println!("holding until {close_at} (funding at {funding_at})");
    loop {
        let remaining = (close_at - Utc::now()).to_std().unwrap_or_default();
        if remaining.is_zero() {
            return;
        }
        if remaining.as_secs() % 600 < 60 {
            println!("{} minutes to close", remaining.as_secs() / 60);
        }
        tokio::time::sleep(remaining.min(Duration::from_secs(60))).await;
    }
}

fn print_cycle(verb: &str, cycle: &SandboxCycle) {
    println!(
        "{verb} cycle {} on {} ({}): quantity {}, spot filled {} closed {}, perp filled {} closed {}, funding at {}",
        cycle.id,
        cycle.venue,
        cycle.state,
        cycle.base_quantity,
        cycle.spot_filled,
        cycle.spot_closed,
        cycle.perp_filled,
        cycle.perp_closed,
        cycle
            .funding_at
            .map_or_else(|| "unknown".into(), |at| at.to_rfc3339()),
    );
}

/// A stable, non-reversible label for an account id.
fn hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .take(6)
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

async fn build_report(
    pool: &PgPool,
    tenant: TenantContext,
    venue: &str,
    since: DateTime<Utc>,
    runs: &[SandboxCycle],
    legs: &SandboxVenue<'_>,
) -> Result<Value, Box<dyn Error>> {
    let client_ids = runs
        .iter()
        .flat_map(|run| {
            [
                run.spot_open_client_id.clone(),
                Some(run.perp_open_client_id.clone()),
                run.spot_close_client_id.clone(),
                run.perp_close_client_id.clone(),
            ]
        })
        .flatten()
        .collect::<Vec<_>>();
    let accounts = vec![legs.spot.account.0.clone(), legs.perp.account.0.clone()];
    let until = runs
        .iter()
        .filter_map(|run| run.closed_at)
        .max()
        .unwrap_or_else(Utc::now)
        + chrono::Duration::minutes(10);
    let mut transaction = TenantTransaction::begin(pool, tenant).await?;
    let tenant_id = tenant.tenant_id().as_uuid();

    let intents = sqlx::query(
        "SELECT client_order_id, venue_order_id, venue_account_id, symbol, product, side,
                base_quantity::text AS quantity, limit_price::text AS price, reduce_only, state,
                error_code, created_at, updated_at
         FROM connector_order_intents
         WHERE tenant_id = $1 AND client_order_id = ANY($2) ORDER BY created_at",
    )
    .bind(tenant_id)
    .bind(&client_ids)
    .fetch_all(transaction.connection())
    .await?;
    let order_ids = intents
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("venue_order_id"))
        .collect::<Vec<_>>();
    let intents = intents
        .iter()
        .map(|row| {
            json!({
                "client_order_id": row.get::<String, _>("client_order_id"),
                "venue_order_id": row.get::<Option<String>, _>("venue_order_id"),
                "account": hash(&row.get::<String, _>("venue_account_id")),
                "symbol": row.get::<String, _>("symbol"),
                "product": row.get::<String, _>("product"),
                "side": row.get::<String, _>("side"),
                "quantity": row.get::<String, _>("quantity"),
                "limit_price": row.get::<String, _>("price"),
                "reduce_only": row.get::<bool, _>("reduce_only"),
                "state": row.get::<String, _>("state"),
                "error_code": row.get::<Option<String>, _>("error_code"),
                "created_at": row.get::<DateTime<Utc>, _>("created_at"),
                "updated_at": row.get::<DateTime<Utc>, _>("updated_at"),
            })
        })
        .collect::<Vec<_>>();

    let fills = sqlx::query(
        "SELECT venue_order_id, symbol, product, quantity::text AS quantity, price::text AS price,
                fee::text AS fee, fee_asset, occurred_at
         FROM connector_fills
         WHERE tenant_id = $1 AND venue = $2 AND venue_account_id = ANY($3)
           AND venue_order_id = ANY($4) ORDER BY occurred_at",
    )
    .bind(tenant_id)
    .bind(venue)
    .bind(&accounts)
    .bind(&order_ids)
    .fetch_all(transaction.connection())
    .await?
    .iter()
    .map(|row| {
        json!({
            "venue_order_id": row.get::<String, _>("venue_order_id"),
            "symbol": row.get::<String, _>("symbol"),
            "product": row.get::<String, _>("product"),
            "quantity": row.get::<String, _>("quantity"),
            "price": row.get::<String, _>("price"),
            "fee": row.get::<String, _>("fee"),
            "fee_asset": row.get::<String, _>("fee_asset"),
            "occurred_at": row.get::<DateTime<Utc>, _>("occurred_at"),
        })
    })
    .collect::<Vec<_>>();

    let funding = sqlx::query(
        "SELECT symbol, amount::text AS amount, asset, occurred_at, venue_entry_id
         FROM connector_funding_ledger
         WHERE tenant_id = $1 AND venue = $2 AND venue_account_id = $3
           AND occurred_at >= $4 AND occurred_at <= $5 ORDER BY occurred_at",
    )
    .bind(tenant_id)
    .bind(venue)
    .bind(&legs.perp.account.0)
    .bind(since)
    .bind(until)
    .fetch_all(transaction.connection())
    .await?
    .iter()
    .map(|row| {
        json!({
            "symbol": row.get::<String, _>("symbol"),
            "amount": row.get::<String, _>("amount"),
            "asset": row.get::<String, _>("asset"),
            "occurred_at": row.get::<DateTime<Utc>, _>("occurred_at"),
            "venue_entry_id": row.get::<String, _>("venue_entry_id"),
        })
    })
    .collect::<Vec<_>>();

    let reconciliation = sqlx::query(
        "SELECT r.id, r.venue_account_id, r.status, r.started_at, r.completed_at, r.fills_seen,
                count(d.id) AS discrepancies,
                coalesce(string_agg(DISTINCT d.kind || ':' || d.severity, ', '), '') AS kinds
         FROM connector_reconciliation_runs r
         LEFT JOIN connector_discrepancies d ON d.run_id = r.id AND d.tenant_id = r.tenant_id
         WHERE r.tenant_id = $1 AND r.venue = $2 AND r.venue_account_id = ANY($3)
           AND r.started_at >= $4
         GROUP BY r.id ORDER BY r.started_at",
    )
    .bind(tenant_id)
    .bind(venue)
    .bind(&accounts)
    .bind(since)
    .fetch_all(transaction.connection())
    .await?
    .iter()
    .map(|row| {
        json!({
            "account": hash(&row.get::<String, _>("venue_account_id")),
            "status": row.get::<String, _>("status"),
            "started_at": row.get::<DateTime<Utc>, _>("started_at"),
            "completed_at": row.get::<Option<DateTime<Utc>>, _>("completed_at"),
            "fills_seen": row.get::<i32, _>("fills_seen"),
            "discrepancies": row.get::<i64, _>("discrepancies"),
            "discrepancy_kinds": row.get::<String, _>("kinds"),
        })
    })
    .collect::<Vec<_>>();
    transaction.commit().await?;

    let runs = runs
        .iter()
        .map(|run| {
            json!({
                "id": run.id,
                "kind": run.kind,
                "state": run.state,
                "underlying": run.underlying,
                "spot_symbol": run.spot_symbol,
                "perp_symbol": run.perp_symbol,
                "base_quantity": run.base_quantity.to_string(),
                "spot_open_client_id": run.spot_open_client_id,
                "perp_open_client_id": run.perp_open_client_id,
                "spot_close_client_id": run.spot_close_client_id,
                "perp_close_client_id": run.perp_close_client_id,
                "spot_filled": run.spot_filled.to_string(),
                "perp_filled": run.perp_filled.to_string(),
                "spot_closed": run.spot_closed.to_string(),
                "perp_closed": run.perp_closed.to_string(),
                "funding_at": run.funding_at,
                "created_at": run.created_at,
                "opened_at": run.opened_at,
                "closed_at": run.closed_at,
                "snapshot_before": run.snapshot_before,
                "snapshot_after": run.snapshot_after,
                "outcome": run.outcome,
            })
        })
        .collect::<Vec<_>>();
    let accounting = accounting(&fills, &funding);
    Ok(json!({
        "venue": venue,
        "environment": legs.environment.as_str(),
        "spot_account": hash(&legs.spot.account.0),
        "perp_account": hash(&legs.perp.account.0),
        "window": {"since": since, "until": until},
        "generated_at": Utc::now(),
        "runs": runs,
        "intents": intents,
        "fills": fills,
        "funding": funding,
        "reconciliation": reconciliation,
        "accounting": accounting,
    }))
}

/// Trading cash flow by asset: sells add quote, buys spend it, fees are
/// charged in their own asset, and funding adds in its asset. Fills do not
/// carry a side, so the side is joined from the intents through the order id.
fn accounting(fills: &[Value], funding: &[Value]) -> Value {
    let decimal = |value: &Value| {
        value
            .as_str()
            .and_then(|text| text.parse::<Decimal>().ok())
            .unwrap_or_default()
    };
    let mut fees = std::collections::BTreeMap::<String, Decimal>::new();
    for fill in fills {
        let asset = fill["fee_asset"].as_str().unwrap_or("?").to_owned();
        *fees.entry(asset).or_default() += decimal(&fill["fee"]);
    }
    let mut received = std::collections::BTreeMap::<String, Decimal>::new();
    for entry in funding {
        let asset = entry["asset"].as_str().unwrap_or("?").to_owned();
        *received.entry(asset).or_default() += decimal(&entry["amount"]);
    }
    let strings = |map: std::collections::BTreeMap<String, Decimal>| {
        map.into_iter()
            .map(|(asset, amount)| (asset, Value::String(amount.to_string())))
            .collect::<serde_json::Map<_, _>>()
    };
    json!({"fees": strings(fees), "funding": strings(received)})
}

fn render(report: &Value) -> String {
    let text = |value: &Value| match value {
        Value::Null => "—".to_owned(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut out = String::new();
    let venue = text(&report["venue"]);
    let _ = writeln!(out, "# M4 sandbox evidence: {venue}\n");
    let _ = writeln!(
        out,
        "- **Environment:** {}\n- **Accounts (hashed):** spot `{}`, perpetual `{}`\n- **Window:** {} to {}\n- **Generated:** {}\n\nThe JSON file beside this one holds every field shown here, plus the account snapshots.\n",
        text(&report["environment"]),
        text(&report["spot_account"]),
        text(&report["perp_account"]),
        text(&report["window"]["since"]),
        text(&report["window"]["until"]),
        text(&report["generated_at"]),
    );
    let _ = writeln!(out, "## Runs\n");
    let _ = writeln!(
        out,
        "| Kind | State | Quantity | Spot filled / closed | Perp filled / closed | Funding at | Opened | Closed |\n|---|---|---|---|---|---|---|---|"
    );
    for run in report["runs"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} / {} | {} / {} | {} | {} | {} |",
            text(&run["kind"]),
            text(&run["state"]),
            text(&run["base_quantity"]),
            text(&run["spot_filled"]),
            text(&run["spot_closed"]),
            text(&run["perp_filled"]),
            text(&run["perp_closed"]),
            text(&run["funding_at"]),
            text(&run["opened_at"]),
            text(&run["closed_at"]),
        );
        if let Some(outcome) = run["outcome"].as_object() {
            let _ = writeln!(
                out,
                "\n**Recovery `{}`:** first submission `{}`, retry with the same client order id `{}` (not sent to the venue), resolved by query to `{}`, final `{}`, filled `{}`, flattened `{}`.\n",
                text(&outcome["client_order_id"]),
                text(&outcome["first_submission"]),
                text(&outcome["retry_submission"]),
                text(&outcome["resolved_venue_status"]),
                text(&outcome["final_venue_status"]),
                text(&outcome["filled"]),
                text(&outcome["flattened"]),
            );
        }
    }
    let _ = writeln!(out, "\n## Orders\n");
    let _ = writeln!(
        out,
        "| Client order id | Symbol | Side | Reduce only | Quantity | Limit | State | Error |\n|---|---|---|---|---|---|---|---|"
    );
    for intent in report["intents"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} | {} |",
            text(&intent["client_order_id"]),
            text(&intent["symbol"]),
            text(&intent["side"]),
            text(&intent["reduce_only"]),
            text(&intent["quantity"]),
            text(&intent["limit_price"]),
            text(&intent["state"]),
            text(&intent["error_code"]),
        );
    }
    let _ = writeln!(out, "\n## Fills\n");
    let _ = writeln!(
        out,
        "| Order | Symbol | Quantity | Price | Fee | Time |\n|---|---|---|---|---|---|"
    );
    for fill in report["fills"].as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} {} | {} |",
            text(&fill["venue_order_id"]),
            text(&fill["symbol"]),
            text(&fill["quantity"]),
            text(&fill["price"]),
            text(&fill["fee"]),
            text(&fill["fee_asset"]),
            text(&fill["occurred_at"]),
        );
    }
    let _ = writeln!(out, "\n## Funding\n");
    let funding = report["funding"].as_array().cloned().unwrap_or_default();
    if funding.is_empty() {
        let _ = writeln!(out, "No funding entry was posted in the window.");
    } else {
        let _ = writeln!(out, "| Symbol | Amount | Time |\n|---|---|---|");
        for entry in funding {
            let _ = writeln!(
                out,
                "| {} | {} {} | {} |",
                text(&entry["symbol"]),
                text(&entry["amount"]),
                text(&entry["asset"]),
                text(&entry["occurred_at"]),
            );
        }
    }
    let _ = writeln!(out, "\n## Accounting\n");
    for (label, key) in [("Fees paid", "fees"), ("Funding received", "funding")] {
        let parts = report["accounting"][key]
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(asset, amount)| format!("{} {asset}", text(amount)))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|joined| !joined.is_empty())
            .unwrap_or_else(|| "none".into());
        let _ = writeln!(out, "- **{label}:** {parts}");
    }
    let _ = writeln!(out, "\n## Reconciliation\n");
    let _ = writeln!(
        out,
        "| Account | Status | Started | Fills seen | Discrepancies |\n|---|---|---|---|---|"
    );
    for run in report["reconciliation"].as_array().into_iter().flatten() {
        let kinds = text(&run["discrepancy_kinds"]);
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {}{} |",
            text(&run["account"]),
            text(&run["status"]),
            text(&run["started_at"]),
            text(&run["fills_seen"]),
            text(&run["discrepancies"]),
            if kinds.is_empty() {
                String::new()
            } else {
                format!(" ({kinds})")
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_labels_are_stable_and_short() {
        assert_eq!(hash("account-1"), hash("account-1"));
        assert_ne!(hash("account-1"), hash("account-2"));
        assert_eq!(hash("account-1").len(), 12);
    }

    #[test]
    fn accounting_sums_fees_and_funding_per_asset_exactly() {
        let fills = vec![
            json!({"fee": "0.00005", "fee_asset": "ETH"}),
            json!({"fee": "0.1499", "fee_asset": "USDT"}),
            json!({"fee": "0.1501", "fee_asset": "USDT"}),
        ];
        let funding = vec![json!({"amount": "0.015", "asset": "USDT"})];
        let summary = accounting(&fills, &funding);
        assert_eq!(summary["fees"]["USDT"], "0.3000");
        assert_eq!(summary["fees"]["ETH"], "0.00005");
        assert_eq!(summary["funding"]["USDT"], "0.015");
    }

    #[test]
    fn the_report_says_so_when_no_funding_posted() {
        let report = json!({
            "venue": "BYBIT", "environment": "TESTNET", "spot_account": "a", "perp_account": "b",
            "window": {"since": "s", "until": "u"}, "generated_at": "g",
            "runs": [], "intents": [], "fills": [], "funding": [], "reconciliation": [],
            "accounting": {"fees": {}, "funding": {}},
        });
        let markdown = render(&report);
        assert!(markdown.starts_with("# M4 sandbox evidence: BYBIT"));
        assert!(markdown.contains("No funding entry was posted in the window."));
        assert!(markdown.contains("- **Fees paid:** none"));
    }
}
