//! Durable submit-once coordinator for sandbox connector writes.

use funding_arb_connectors::{
    AccountId, CanonicalOrderIntent, ConnectorError, ConnectorErrorKind, OrderAck, OrderLookup,
    OrderStatus, PreparedOrder, VenueConnector, VenueEnvironment, VenueOrder,
};
use funding_arb_domain::TradingMode;
use sqlx::PgPool;
use std::{error::Error, fmt};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubmissionState {
    Duplicate,
    Acknowledged,
    UnknownOutcome,
    Failed,
}

#[derive(Clone, Debug)]
pub struct SubmissionResult {
    pub intent_id: Uuid,
    pub state: SubmissionState,
    pub acknowledgement: Option<OrderAck>,
}

#[derive(Debug)]
pub enum CoordinatorError {
    ModeForbidden,
    ProductionWriteForbidden,
    InvalidPreparedOrder,
    Persistence,
    Connector(ConnectorError),
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ModeForbidden => "runtime mode forbids connector order submission",
            Self::ProductionWriteForbidden => "production connector writes are forbidden",
            Self::InvalidPreparedOrder => "connector returned an invalid prepared order",
            Self::Persistence => "connector execution persistence failed",
            Self::Connector(_) => "connector execution failed",
        })
    }
}

impl Error for CoordinatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connector(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct ConnectorExecutionCoordinator {
    pool: PgPool,
}

impl ConnectorExecutionCoordinator {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Normalize and hash a native order without performing a venue write.
    pub async fn preview(
        &self,
        connector: &dyn VenueConnector,
        order: &CanonicalOrderIntent,
    ) -> Result<PreparedOrder, CoordinatorError> {
        let prepared = connector
            .prepare_order(order)
            .await
            .map_err(CoordinatorError::Connector)?;
        if prepared.payload_hash.trim().is_empty()
            || prepared.native_quantity <= rust_decimal::Decimal::ZERO
            || prepared.native_price <= rust_decimal::Decimal::ZERO
        {
            return Err(CoordinatorError::InvalidPreparedOrder);
        }
        Ok(prepared)
    }

    /// Persist before submit and never submit a duplicate client order id.
    pub async fn submit_sandbox(
        &self,
        mode: TradingMode,
        environment: VenueEnvironment,
        connector: &dyn VenueConnector,
        order: &CanonicalOrderIntent,
    ) -> Result<SubmissionResult, CoordinatorError> {
        if !mode.allows_external_writes() {
            return Err(CoordinatorError::ModeForbidden);
        }
        if !environment.is_non_production() {
            return Err(CoordinatorError::ProductionWriteForbidden);
        }
        let prepared = self.preview(connector, order).await?;
        let inserted = self.insert_prepared(environment, &prepared).await?;
        let Some(intent_id) = inserted else {
            let intent_id = self.existing_id(&prepared).await?;
            return Ok(SubmissionResult {
                intent_id,
                state: SubmissionState::Duplicate,
                acknowledgement: None,
            });
        };
        match connector.submit_order(&prepared).await {
            Ok(acknowledgement) => {
                self.record_ack(intent_id, &acknowledgement).await?;
                Ok(SubmissionResult {
                    intent_id,
                    state: SubmissionState::Acknowledged,
                    acknowledgement: Some(acknowledgement),
                })
            }
            Err(error) if error.kind == ConnectorErrorKind::UnknownOutcome => {
                self.mark_error(intent_id, "UNKNOWN_OUTCOME", &error.code)
                    .await?;
                Ok(SubmissionResult {
                    intent_id,
                    state: SubmissionState::UnknownOutcome,
                    acknowledgement: None,
                })
            }
            Err(error) => {
                self.mark_error(intent_id, "FAILED", &error.code).await?;
                Err(CoordinatorError::Connector(error))
            }
        }
    }

    /// Query-before-retry recovery for an ambiguous submission.
    pub async fn reconcile_unknown(
        &self,
        connector: &dyn VenueConnector,
        account: &AccountId,
        client_order_id: &str,
    ) -> Result<Option<VenueOrder>, CoordinatorError> {
        let order = connector
            .query_order(
                account,
                &OrderLookup::ClientOrderId(client_order_id.to_owned()),
            )
            .await
            .map_err(CoordinatorError::Connector)?;
        if let Some(venue_order) = &order {
            self.record_reconciled(account, client_order_id, venue_order)
                .await?;
        }
        Ok(order)
    }

    async fn insert_prepared(
        &self,
        environment: VenueEnvironment,
        prepared: &PreparedOrder,
    ) -> Result<Option<Uuid>, CoordinatorError> {
        let id = Uuid::new_v4();
        sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO connector_order_intents
             (id, venue, venue_account_id, client_order_id, symbol, product, side,
              base_quantity, limit_price, native_quantity, native_price,
              equivalent_base_quantity, projected_delta_usd, reduce_only, environment,
              state, prepared_payload, payload_hash)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8::numeric, $9::numeric,
                     $10::numeric, $11::numeric, $12::numeric, $13::numeric, $14, $15,
                     'SUBMITTING', $16, $17)
             ON CONFLICT (venue, venue_account_id, client_order_id) DO NOTHING
             RETURNING id",
        )
        .bind(id)
        .bind(prepared.intent.instrument.venue.as_str())
        .bind(&prepared.intent.account_id.0)
        .bind(&prepared.intent.client_order_id)
        .bind(&prepared.intent.instrument.symbol)
        .bind(prepared.intent.instrument.product.as_str())
        .bind(side_wire(prepared.intent.side))
        .bind(prepared.intent.base_quantity.to_string())
        .bind(prepared.intent.limit_price.to_string())
        .bind(prepared.native_quantity.to_string())
        .bind(prepared.native_price.to_string())
        .bind(prepared.equivalent_base_quantity.to_string())
        .bind(prepared.projected_delta_usd.to_string())
        .bind(prepared.intent.reduce_only)
        .bind(environment.as_str())
        .bind(prepared.payload.to_string())
        .bind(&prepared.payload_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CoordinatorError::Persistence)
    }

    async fn existing_id(&self, prepared: &PreparedOrder) -> Result<Uuid, CoordinatorError> {
        sqlx::query_scalar(
            "SELECT id FROM connector_order_intents
             WHERE venue = $1 AND venue_account_id = $2 AND client_order_id = $3",
        )
        .bind(prepared.intent.instrument.venue.as_str())
        .bind(&prepared.intent.account_id.0)
        .bind(&prepared.intent.client_order_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| CoordinatorError::Persistence)
    }

    async fn record_ack(
        &self,
        intent_id: Uuid,
        acknowledgement: &OrderAck,
    ) -> Result<(), CoordinatorError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| CoordinatorError::Persistence)?;
        sqlx::query(
            "UPDATE connector_order_intents
             SET venue_order_id = $2, state = $3, updated_at = now(), error_code = NULL
             WHERE id = $1",
        )
        .bind(intent_id)
        .bind(&acknowledgement.venue_order_id)
        .bind(status_wire(acknowledgement.status))
        .execute(&mut *transaction)
        .await
        .map_err(|_| CoordinatorError::Persistence)?;
        sqlx::query(
            "INSERT INTO connector_order_events
             (id, intent_id, status, venue_order_id, received_at, payload)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(intent_id)
        .bind(status_wire(acknowledgement.status))
        .bind(&acknowledgement.venue_order_id)
        .bind(acknowledgement.acknowledged_at)
        .bind(format!(
            "{{\"client_order_id\":{}}}",
            serde_json::to_string(&acknowledgement.client_order_id)
                .map_err(|_| CoordinatorError::Persistence)?
        ))
        .execute(&mut *transaction)
        .await
        .map_err(|_| CoordinatorError::Persistence)?;
        transaction
            .commit()
            .await
            .map_err(|_| CoordinatorError::Persistence)
    }

    async fn mark_error(
        &self,
        intent_id: Uuid,
        state: &str,
        error_code: &str,
    ) -> Result<(), CoordinatorError> {
        sqlx::query(
            "UPDATE connector_order_intents
             SET state = $2, error_code = $3, updated_at = now() WHERE id = $1",
        )
        .bind(intent_id)
        .bind(state)
        .bind(error_code)
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|_| CoordinatorError::Persistence)
    }

    async fn record_reconciled(
        &self,
        account: &AccountId,
        client_order_id: &str,
        order: &VenueOrder,
    ) -> Result<(), CoordinatorError> {
        let status = status_wire(order.status);
        let row = sqlx::query_scalar::<_, Uuid>(
            "UPDATE connector_order_intents
             SET venue_order_id = $4, state = $5, error_code = NULL, updated_at = now()
             WHERE venue = $1 AND venue_account_id = $2 AND client_order_id = $3
               AND state = 'UNKNOWN_OUTCOME'
             RETURNING id",
        )
        .bind(order.instrument.venue.as_str())
        .bind(&account.0)
        .bind(client_order_id)
        .bind(&order.venue_order_id)
        .bind(status)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CoordinatorError::Persistence)?;
        if let Some(intent_id) = row {
            sqlx::query(
                "INSERT INTO connector_order_events
                 (id, intent_id, status, venue_order_id, venue_sequence, venue_updated_at,
                  received_at, payload)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, '{}')
                 ON CONFLICT DO NOTHING",
            )
            .bind(Uuid::new_v4())
            .bind(intent_id)
            .bind(status)
            .bind(&order.venue_order_id)
            .bind(order.sequence.and_then(|value| i64::try_from(value).ok()))
            .bind(order.venue_updated_at)
            .bind(order.received_at)
            .execute(&self.pool)
            .await
            .map_err(|_| CoordinatorError::Persistence)?;
        }
        Ok(())
    }
}

const fn side_wire(side: funding_arb_connectors::OrderSide) -> &'static str {
    match side {
        funding_arb_connectors::OrderSide::Buy => "BUY",
        funding_arb_connectors::OrderSide::Sell => "SELL",
    }
}

const fn status_wire(status: OrderStatus) -> &'static str {
    match status {
        OrderStatus::Pending => "PENDING",
        OrderStatus::Open => "OPEN",
        OrderStatus::PartiallyFilled => "PARTIALLY_FILLED",
        OrderStatus::Filled => "FILLED",
        OrderStatus::Cancelled => "CANCELLED",
        OrderStatus::Rejected => "REJECTED",
        OrderStatus::Unknown => "UNKNOWN_OUTCOME",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_and_non_sandbox_writes_fail_closed() {
        assert!(!TradingMode::Shadow.allows_external_writes());
        assert!(!TradingMode::Paper.allows_external_writes());
        assert!(!VenueEnvironment::Production.is_non_production());
    }

    #[test]
    fn coordinator_error_does_not_render_connector_detail() {
        let error = CoordinatorError::Connector(ConnectorError {
            kind: ConnectorErrorKind::FatalConfiguration,
            code: "SECRET".into(),
            message: "raw credential detail".into(),
            retry_after_ms: None,
        });
        assert_eq!(error.to_string(), "connector execution failed");
    }
}
