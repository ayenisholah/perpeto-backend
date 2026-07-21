//! Read-only venue reconciliation and exact accounting snapshots.

use std::{collections::HashSet, error::Error, fmt};

use chrono::{Duration, Utc};
use funding_arb_connectors::{
    AccountId, Balance, Fill, FundingLedgerEntry, OrderLookup, TimeRange, VenueConnector,
    VenueOrder, VenuePosition,
};
use funding_arb_tenant_context::{TenantContext, TenantTransaction};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationReport {
    pub run_id: Uuid,
    pub status: String,
    pub balances_seen: usize,
    pub positions_seen: usize,
    pub open_orders_seen: usize,
    pub fills_seen: usize,
    pub funding_entries_seen: usize,
    pub discrepancies: usize,
}

#[derive(Debug)]
pub enum ReconciliationError {
    Connector,
    Persistence,
    PaginationLimit,
}

impl fmt::Display for ReconciliationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connector => "venue reconciliation read failed",
            Self::Persistence => "venue reconciliation persistence failed",
            Self::PaginationLimit => "venue fill pagination exceeded the safety limit",
        })
    }
}

impl Error for ReconciliationError {}

#[derive(Clone)]
pub struct ReconciliationService {
    pool: PgPool,
}

impl ReconciliationService {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Take a read-only external snapshot and compare unresolved local orders.
    pub async fn run(
        &self,
        context: TenantContext,
        connector: &dyn VenueConnector,
        account: &AccountId,
    ) -> Result<ReconciliationReport, ReconciliationError> {
        let run_id = Uuid::new_v4();
        let started_at = Utc::now();
        let venue = connector.venue().as_str();
        self.start_run(context, run_id, venue, account, started_at)
            .await?;

        let balances = connector
            .balances(account)
            .await
            .map_err(|_| ReconciliationError::Connector)?;
        let positions = connector
            .positions(account)
            .await
            .map_err(|_| ReconciliationError::Connector)?;
        let open_orders = connector
            .open_orders(account)
            .await
            .map_err(|_| ReconciliationError::Connector)?;
        let fills = self.all_fills(connector, account).await?;
        let range = TimeRange {
            start: started_at - Duration::days(30),
            end: started_at,
        };
        let funding = connector
            .funding_ledger(account, &range)
            .await
            .map_err(|_| ReconciliationError::Connector)?;
        let discrepancies = self
            .find_discrepancies(context, run_id, connector, account, &open_orders)
            .await?;
        self.persist_snapshot(
            context, run_id, venue, account, &balances, &positions, &fills, &funding,
        )
        .await?;
        let status = if discrepancies == 0 {
            "RECONCILED"
        } else {
            "REQUIRED"
        };
        self.complete_run(
            context,
            run_id,
            status,
            balances.len(),
            positions.len(),
            open_orders.len(),
            fills.len(),
        )
        .await?;
        Ok(ReconciliationReport {
            run_id,
            status: status.into(),
            balances_seen: balances.len(),
            positions_seen: positions.len(),
            open_orders_seen: open_orders.len(),
            fills_seen: fills.len(),
            funding_entries_seen: funding.len(),
            discrepancies,
        })
    }

    async fn all_fills(
        &self,
        connector: &dyn VenueConnector,
        account: &AccountId,
    ) -> Result<Vec<Fill>, ReconciliationError> {
        let mut cursor = None;
        let mut fills = Vec::new();
        for _ in 0..100 {
            let page = connector
                .fills(account, cursor.as_deref())
                .await
                .map_err(|_| ReconciliationError::Connector)?;
            fills.extend(page.fills);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(fills);
            }
        }
        Err(ReconciliationError::PaginationLimit)
    }

    async fn find_discrepancies(
        &self,
        context: TenantContext,
        run_id: Uuid,
        connector: &dyn VenueConnector,
        account: &AccountId,
        external_open: &[VenueOrder],
    ) -> Result<usize, ReconciliationError> {
        let mut transaction = TenantTransaction::begin(&self.pool, context)
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        let local = sqlx::query_as::<_, (String,)>(
            "SELECT client_order_id FROM connector_order_intents
             WHERE tenant_id = $1 AND venue = $2 AND venue_account_id = $3
               AND state IN ('SUBMITTING', 'UNKNOWN_OUTCOME', 'OPEN', 'PARTIALLY_FILLED')",
        )
        .bind(context.tenant_id().as_uuid())
        .bind(connector.venue().as_str())
        .bind(&account.0)
        .fetch_all(transaction.connection())
        .await
        .map_err(|_| ReconciliationError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        let local = local.into_iter().map(|row| row.0).collect::<HashSet<_>>();
        let external = external_open
            .iter()
            .filter_map(|order| order.client_order_id.clone())
            .collect::<HashSet<_>>();
        let mut count = 0;
        for client_order_id in external.difference(&local) {
            self.discrepancy(
                context,
                run_id,
                "EXTERNAL_ORDER",
                "CRITICAL",
                client_order_id,
            )
            .await?;
            count += 1;
        }
        for client_order_id in local.difference(&external) {
            let found = connector
                .query_order(
                    account,
                    &OrderLookup::ClientOrderId(client_order_id.clone()),
                )
                .await
                .map_err(|_| ReconciliationError::Connector)?;
            if found.is_none() {
                self.discrepancy(
                    context,
                    run_id,
                    "LOCAL_ORDER_MISSING",
                    "CRITICAL",
                    client_order_id,
                )
                .await?;
                count += 1;
            }
        }
        Ok(count)
    }

    async fn start_run(
        &self,
        context: TenantContext,
        run_id: Uuid,
        venue: &str,
        account: &AccountId,
        started_at: chrono::DateTime<Utc>,
    ) -> Result<(), ReconciliationError> {
        let mut transaction = TenantTransaction::begin(&self.pool, context)
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        sqlx::query(
            "INSERT INTO connector_reconciliation_runs
             (tenant_id, id, venue, venue_account_id, status, started_at, payload)
             VALUES ($1, $2, $3, $4, 'RUNNING', $5, '{}')",
        )
        .bind(context.tenant_id().as_uuid())
        .bind(run_id)
        .bind(venue)
        .bind(&account.0)
        .bind(started_at)
        .execute(transaction.connection())
        .await
        .map_err(|_| ReconciliationError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| ReconciliationError::Persistence)
    }

    async fn complete_run(
        &self,
        context: TenantContext,
        run_id: Uuid,
        status: &str,
        balances: usize,
        positions: usize,
        open_orders: usize,
        fills: usize,
    ) -> Result<(), ReconciliationError> {
        let mut transaction = TenantTransaction::begin(&self.pool, context)
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        sqlx::query(
            "UPDATE connector_reconciliation_runs
             SET status = $2, completed_at = now(), balances_seen = $3,
                 positions_seen = $4, open_orders_seen = $5, fills_seen = $6,
                 payload = $7 WHERE tenant_id = $8 AND id = $1",
        )
        .bind(run_id)
        .bind(status)
        .bind(count_i32(balances)?)
        .bind(count_i32(positions)?)
        .bind(count_i32(open_orders)?)
        .bind(count_i32(fills)?)
        .bind(json!({ "status": status }).to_string())
        .bind(context.tenant_id().as_uuid())
        .execute(transaction.connection())
        .await
        .map_err(|_| ReconciliationError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| ReconciliationError::Persistence)
    }

    async fn discrepancy(
        &self,
        context: TenantContext,
        run_id: Uuid,
        kind: &str,
        severity: &str,
        external_key: &str,
    ) -> Result<(), ReconciliationError> {
        let mut transaction = TenantTransaction::begin(&self.pool, context)
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        sqlx::query(
            "INSERT INTO connector_discrepancies
             (tenant_id, id, run_id, kind, severity, external_key, status, detected_at, payload)
             VALUES ($1, $2, $3, $4, $5, $6, 'OPEN', now(), '{}')
             ON CONFLICT DO NOTHING",
        )
        .bind(context.tenant_id().as_uuid())
        .bind(Uuid::new_v4())
        .bind(run_id)
        .bind(kind)
        .bind(severity)
        .bind(external_key)
        .execute(transaction.connection())
        .await
        .map_err(|_| ReconciliationError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| ReconciliationError::Persistence)
    }

    async fn persist_snapshot(
        &self,
        context: TenantContext,
        run_id: Uuid,
        venue: &str,
        account: &AccountId,
        balances: &[Balance],
        positions: &[VenuePosition],
        fills: &[Fill],
        funding: &[FundingLedgerEntry],
    ) -> Result<(), ReconciliationError> {
        let mut transaction = TenantTransaction::begin(&self.pool, context)
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        for balance in balances {
            sqlx::query(
                "INSERT INTO connector_balance_snapshots
                 (tenant_id, id, run_id, venue, venue_account_id, asset, total, available, locked,
                  venue_updated_at, received_at, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7::numeric, $8::numeric, $9::numeric,
                         $10, $11, $12)",
            )
            .bind(context.tenant_id().as_uuid())
            .bind(Uuid::new_v4())
            .bind(run_id)
            .bind(venue)
            .bind(&account.0)
            .bind(&balance.asset)
            .bind(balance.total.to_string())
            .bind(balance.available.to_string())
            .bind(balance.locked.to_string())
            .bind(balance.venue_updated_at)
            .bind(balance.received_at)
            .bind(json!({ "asset": balance.asset }).to_string())
            .execute(transaction.connection())
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        }
        for position in positions {
            sqlx::query(
                "INSERT INTO connector_position_snapshots
                 (tenant_id, id, run_id, venue, venue_account_id, symbol, product, quantity, entry_price,
                  mark_price, leverage, liquidation_price, venue_updated_at, received_at, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8::numeric, $9::numeric, $10::numeric,
                         $11::numeric, $12::numeric, $13, $14, $15)",
            )
            .bind(context.tenant_id().as_uuid())
            .bind(Uuid::new_v4())
            .bind(run_id)
            .bind(venue)
            .bind(&account.0)
            .bind(&position.instrument.symbol)
            .bind(position.instrument.product.as_str())
            .bind(position.quantity.to_string())
            .bind(position.entry_price.to_string())
            .bind(position.mark_price.to_string())
            .bind(position.leverage.to_string())
            .bind(position.liquidation_price.map(|value| value.to_string()))
            .bind(position.venue_updated_at)
            .bind(position.received_at)
            .bind(json!({ "symbol": position.instrument.symbol }).to_string())
            .execute(transaction.connection())
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        }
        for fill in fills {
            sqlx::query(
                "INSERT INTO connector_fills
                 (tenant_id, id, venue, venue_account_id, venue_fill_id, venue_order_id, symbol, product,
                  quantity, price, fee, fee_asset, occurred_at, received_at, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::numeric, $10::numeric,
                         $11::numeric, $12, $13, $14, $15)
                 ON CONFLICT (tenant_id, venue, venue_account_id, venue_fill_id) DO NOTHING",
            )
            .bind(context.tenant_id().as_uuid())
            .bind(Uuid::new_v4())
            .bind(venue)
            .bind(&account.0)
            .bind(&fill.venue_fill_id)
            .bind(&fill.venue_order_id)
            .bind(&fill.instrument.symbol)
            .bind(fill.instrument.product.as_str())
            .bind(fill.quantity.to_string())
            .bind(fill.price.to_string())
            .bind(fill.fee.to_string())
            .bind(&fill.fee_asset)
            .bind(fill.occurred_at)
            .bind(fill.received_at)
            .bind(json!({ "venue_fill_id": fill.venue_fill_id }).to_string())
            .execute(transaction.connection())
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        }
        for entry in funding {
            sqlx::query(
                "INSERT INTO connector_funding_ledger
                 (tenant_id, id, venue, venue_account_id, venue_entry_id, symbol, product, amount,
                  asset, occurred_at, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8::numeric, $9, $10, $11)
                 ON CONFLICT (tenant_id, venue, venue_account_id, venue_entry_id) DO NOTHING",
            )
            .bind(context.tenant_id().as_uuid())
            .bind(Uuid::new_v4())
            .bind(venue)
            .bind(&account.0)
            .bind(&entry.venue_entry_id)
            .bind(&entry.instrument.symbol)
            .bind(entry.instrument.product.as_str())
            .bind(entry.amount.to_string())
            .bind(&entry.asset)
            .bind(entry.occurred_at)
            .bind(json!({ "venue_entry_id": entry.venue_entry_id }).to_string())
            .execute(transaction.connection())
            .await
            .map_err(|_| ReconciliationError::Persistence)?;
        }
        transaction
            .commit()
            .await
            .map_err(|_| ReconciliationError::Persistence)
    }
}

fn count_i32(value: usize) -> Result<i32, ReconciliationError> {
    i32::try_from(value).map_err(|_| ReconciliationError::Persistence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_conversion_fails_closed() {
        assert_eq!(count_i32(3).expect("small count"), 3);
        if usize::BITS > 32 {
            assert!(count_i32(usize::MAX).is_err());
        }
    }
}
