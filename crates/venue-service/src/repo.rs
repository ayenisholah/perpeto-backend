//! PostgreSQL serialized-payload venue repository.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{InstrumentDto, ProductConnectionDto, VenueAccountDto};
use funding_arb_connectors::VenueHealth;

#[derive(Clone)]
pub struct VenueAccountsRepo {
    pool: PgPool,
}

impl VenueAccountsRepo {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, account: &VenueAccountDto) -> Result<(), sqlx::Error> {
        let id = parse_uuid(&account.id)?;
        let created_at = parse_time(&account.created_at)?;
        let payload = encode(account)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO venue_accounts \
             (id, venue, alias, enabled, certification_state, reconciliation_state, \
              masked_identity, created_at, payload) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id)
        .bind(&account.venue)
        .bind(&account.alias)
        .bind(account.enabled)
        .bind(certification_wire(account.certification_state))
        .bind(reconciliation_wire(account.reconciliation_state))
        .bind(&account.masked_identity)
        .bind(created_at)
        .bind(payload)
        .execute(&mut *transaction)
        .await?;
        for connection in &account.connections {
            sqlx::query(
                "INSERT INTO venue_product_connections \
                 (id, venue_account_id, product, environment, secret_ref, enabled, payload) \
                 VALUES ($1, $2, $3, $4, $5, true, $6)",
            )
            .bind(parse_uuid(&connection.id)?)
            .bind(id)
            .bind(&connection.product)
            .bind(&connection.environment)
            .bind(&connection.secret_ref)
            .bind(serde_json::to_string(connection).map_err(protocol)?)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await
    }

    pub async fn list(&self) -> Result<Vec<VenueAccountDto>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT payload FROM venue_accounts WHERE tombstoned_at IS NULL \
             ORDER BY created_at, id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| decode(&row.get::<String, _>("payload")))
            .collect()
    }

    pub async fn get(&self, id: &str) -> Result<Option<VenueAccountDto>, sqlx::Error> {
        let row = sqlx::query("SELECT payload FROM venue_accounts WHERE id = $1")
            .bind(parse_uuid(id)?)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|value| decode(&value.get::<String, _>("payload")))
            .transpose()
    }

    pub async fn update(&self, account: &VenueAccountDto) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE venue_accounts SET enabled = $2, certification_state = $3, \
             reconciliation_state = $4, last_reconciled_at = $5, tombstoned_at = $6, \
             payload = $7 WHERE id = $1",
        )
        .bind(parse_uuid(&account.id)?)
        .bind(account.enabled)
        .bind(certification_wire(account.certification_state))
        .bind(reconciliation_wire(account.reconciliation_state))
        .bind(parse_optional_time(account.last_reconciled_at.as_deref())?)
        .bind(parse_optional_time(account.tombstoned_at.as_deref())?)
        .bind(encode(account)?)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    pub async fn update_connection(
        &self,
        account_id: &str,
        connection: &ProductConnectionDto,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE venue_product_connections
             SET secret_ref = $3, enabled = $4, payload = $5
             WHERE id = $1 AND venue_account_id = $2",
        )
        .bind(parse_uuid(&connection.id)?)
        .bind(parse_uuid(account_id)?)
        .bind(&connection.secret_ref)
        .bind(connection.status != "DISABLED")
        .bind(serde_json::to_string(connection).map_err(protocol)?)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    pub async fn upsert_instrument(&self, instrument: &InstrumentDto) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO instruments \
             (venue, symbol, product, canonical_underlying, quote_asset, settlement_asset, \
              margin_asset, contract_multiplier, price_tick, quantity_step, minimum_quantity, \
              maximum_quantity, minimum_notional, funding_interval_hours, funding_cap, \
              funding_floor, maximum_leverage, trading, metadata_version, venue_updated_at, \
              received_at, payload) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8::numeric, $9::numeric, $10::numeric, \
                     $11::numeric, $12::numeric, $13::numeric, $14, $15::numeric, $16::numeric, \
                     $17::numeric, $18, $19, $20, $21, $22) \
             ON CONFLICT (venue, symbol, product) DO UPDATE SET \
               contract_multiplier = EXCLUDED.contract_multiplier, price_tick = EXCLUDED.price_tick, \
               quantity_step = EXCLUDED.quantity_step, minimum_quantity = EXCLUDED.minimum_quantity, \
               maximum_quantity = EXCLUDED.maximum_quantity, minimum_notional = EXCLUDED.minimum_notional, \
               funding_interval_hours = EXCLUDED.funding_interval_hours, funding_cap = EXCLUDED.funding_cap, \
               funding_floor = EXCLUDED.funding_floor, maximum_leverage = EXCLUDED.maximum_leverage, \
               trading = EXCLUDED.trading, metadata_version = EXCLUDED.metadata_version, \
               venue_updated_at = EXCLUDED.venue_updated_at, received_at = EXCLUDED.received_at, \
               payload = EXCLUDED.payload",
        )
        .bind(&instrument.venue)
        .bind(&instrument.symbol)
        .bind(&instrument.product)
        .bind(&instrument.canonical_underlying)
        .bind(&instrument.quote_asset)
        .bind(&instrument.settlement_asset)
        .bind(&instrument.margin_asset)
        .bind(&instrument.contract_multiplier)
        .bind(&instrument.price_tick)
        .bind(&instrument.quantity_step)
        .bind(&instrument.minimum_quantity)
        .bind(&instrument.maximum_quantity)
        .bind(&instrument.minimum_notional)
        .bind(instrument.funding_interval_hours.map(i32::try_from).transpose().map_err(protocol)?)
        .bind(&instrument.funding_cap)
        .bind(&instrument.funding_floor)
        .bind(&instrument.maximum_leverage)
        .bind(instrument.trading)
        .bind(&instrument.metadata_version)
        .bind(parse_optional_time(instrument.venue_updated_at.as_deref())?)
        .bind(parse_time(&instrument.received_at)?)
        .bind(serde_json::to_string(instrument).map_err(protocol)?)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    pub async fn record_health(
        &self,
        account_id: Option<&str>,
        health: &VenueHealth,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO venue_health_samples
             (id, venue_account_id, venue, environment, status, clock_drift_ms,
              rate_limit_remaining, observed_at, payload)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(Uuid::new_v4())
        .bind(account_id.map(parse_uuid).transpose()?)
        .bind(health.venue.as_str())
        .bind(health.environment.as_str())
        .bind(format!("{:?}", health.status).to_ascii_uppercase())
        .bind(health.clock_drift_ms)
        .bind(
            health
                .rate_limit_remaining
                .and_then(|value| i64::try_from(value).ok()),
        )
        .bind(health.observed_at)
        .bind(
            serde_json::json!({
                "venue": health.venue.as_str(),
                "environment": health.environment.as_str(),
                "clock_drift_ms": health.clock_drift_ms,
                "observed_at": health.observed_at.to_rfc3339(),
            })
            .to_string(),
        )
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}

fn certification_wire(value: crate::CertificationState) -> &'static str {
    match value {
        crate::CertificationState::Disabled => "DISABLED",
        crate::CertificationState::PaperOnly => "PAPER_ONLY",
        crate::CertificationState::Shadow => "SHADOW",
        crate::CertificationState::Pilot => "PILOT",
        crate::CertificationState::CertifiedLive => "CERTIFIED_LIVE",
    }
}

fn reconciliation_wire(value: crate::ReconciliationState) -> &'static str {
    match value {
        crate::ReconciliationState::Pending => "PENDING",
        crate::ReconciliationState::Reconciled => "RECONCILED",
        crate::ReconciliationState::Required => "REQUIRED",
        crate::ReconciliationState::Frozen => "FROZEN",
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, sqlx::Error> {
    Uuid::parse_str(value).map_err(protocol)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, sqlx::Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(protocol)
}

fn parse_optional_time(value: Option<&str>) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    value.map(parse_time).transpose()
}

fn encode(value: &VenueAccountDto) -> Result<String, sqlx::Error> {
    serde_json::to_string(value).map_err(protocol)
}

fn decode(value: &str) -> Result<VenueAccountDto, sqlx::Error> {
    serde_json::from_str(value).map_err(protocol)
}

fn protocol(error: impl std::fmt::Display) -> sqlx::Error {
    sqlx::Error::Protocol(error.to_string())
}
