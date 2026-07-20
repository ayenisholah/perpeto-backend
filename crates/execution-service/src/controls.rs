//! Durable circuit breakers and outbox events for paper risk controls.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Scope at which a circuit breaker applies.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BreakerScope {
    /// Every route and strategy.
    Global,
    /// A single venue, identified by `scope_key`.
    Venue,
    /// A single strategy, identified by `scope_key`.
    Strategy,
}

/// Action enforced while a breaker is active.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BreakerAction {
    /// Refuse entries and compounding.
    HaltNewRisk,
    /// Refuse new exposure on one venue and exit affected paper positions.
    DisableVenue,
    /// Refuse every simulated order while continuing observation.
    ReadOnly,
    /// Flatten every paper position, then continue blocking new risk.
    Flatten,
}

/// Durable circuit-breaker representation served by the controls API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CircuitBreakerDto {
    /// UUID string.
    pub id: String,
    /// Breaker scope.
    pub scope: BreakerScope,
    /// Venue or strategy identifier for scoped breakers.
    pub scope_key: Option<String>,
    /// Effective action.
    pub action: BreakerAction,
    /// `OPERATOR`, `DAILY_LOSS`, `DRAWDOWN`, or `VENUE_HEALTH`.
    pub source: String,
    /// Operator/engine-readable reason.
    pub reason: String,
    /// Actor user id for operator controls.
    pub created_by: Option<String>,
    /// Whether the breaker is active.
    pub active: bool,
    /// RFC 3339 activation time.
    pub created_at: String,
    /// RFC 3339 resolution time.
    pub resolved_at: Option<String>,
}

/// Current effective paper controls.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlsDto {
    /// Effective action after applying safety precedence.
    pub effective_action: Option<BreakerAction>,
    /// Active breakers, newest first.
    pub breakers: Vec<CircuitBreakerDto>,
}

/// Cloneable Postgres adapter for breakers.
#[derive(Clone)]
pub struct BreakersRepo {
    pool: PgPool,
}

impl BreakersRepo {
    /// Construct over an existing pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Activate a breaker idempotently for its scope/action.
    #[allow(clippy::too_many_arguments)]
    pub async fn activate(
        &self,
        scope: BreakerScope,
        scope_key: Option<String>,
        action: BreakerAction,
        source: &str,
        reason: String,
        created_by: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<CircuitBreakerDto, sqlx::Error> {
        let dto = CircuitBreakerDto {
            id: Uuid::new_v4().to_string(),
            scope,
            scope_key,
            action,
            source: source.to_owned(),
            reason,
            created_by,
            active: true,
            created_at: now.to_rfc3339(),
            resolved_at: None,
        };
        let payload = serde_json::to_string(&dto)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let inserted = sqlx::query(
            "INSERT INTO circuit_breakers \
             (id, scope, scope_key, action, source, active, created_at, payload) \
             VALUES ($1, $2, $3, $4, $5, true, $6, $7) \
             ON CONFLICT DO NOTHING \
             RETURNING payload",
        )
        .bind(Uuid::parse_str(&dto.id).map_err(|error| sqlx::Error::Protocol(error.to_string()))?)
        .bind(scope_wire(&dto.scope))
        .bind(&dto.scope_key)
        .bind(action_wire(&dto.action))
        .bind(&dto.source)
        .bind(now)
        .bind(&payload)
        .fetch_optional(&self.pool)
        .await?;
        let row = match inserted {
            Some(row) => row,
            None => {
                sqlx::query(
                    "SELECT payload FROM circuit_breakers \
                 WHERE active AND scope = $1 AND scope_key IS NOT DISTINCT FROM $2 AND action = $3 \
                 LIMIT 1",
                )
                .bind(scope_wire(&dto.scope))
                .bind(&dto.scope_key)
                .bind(action_wire(&dto.action))
                .fetch_one(&self.pool)
                .await?
            }
        };
        decode(&row.get::<String, _>("payload"))
    }

    /// Return active breakers and their effective action.
    pub async fn controls(&self) -> Result<ControlsDto, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT payload FROM circuit_breakers WHERE active ORDER BY created_at DESC, id DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let breakers = rows
            .iter()
            .map(|row| decode(&row.get::<String, _>("payload")))
            .collect::<Result<Vec<_>, _>>()?;
        let effective_action = effective_action(&breakers);
        Ok(ControlsDto {
            effective_action,
            breakers,
        })
    }

    /// Resolve every operator-created breaker; automatic breakers remain.
    pub async fn resolve_operator(&self, now: DateTime<Utc>) -> Result<u64, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, payload FROM circuit_breakers WHERE active AND source = 'OPERATOR' FOR UPDATE",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut resolved = 0;
        for row in rows {
            let id: Uuid = row.get("id");
            let mut dto = decode(&row.get::<String, _>("payload"))?;
            dto.active = false;
            dto.resolved_at = Some(now.to_rfc3339());
            let payload = serde_json::to_string(&dto)
                .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
            resolved += sqlx::query(
                "UPDATE circuit_breakers SET active = false, resolved_at = $2, payload = $3 WHERE id = $1",
            )
            .bind(id)
            .bind(now)
            .bind(payload)
            .execute(&self.pool)
            .await?
            .rows_affected();
        }
        Ok(resolved)
    }
}

fn decode(payload: &str) -> Result<CircuitBreakerDto, sqlx::Error> {
    serde_json::from_str(payload).map_err(|error| sqlx::Error::Protocol(error.to_string()))
}

fn scope_wire(scope: &BreakerScope) -> &'static str {
    match scope {
        BreakerScope::Global => "GLOBAL",
        BreakerScope::Venue => "VENUE",
        BreakerScope::Strategy => "STRATEGY",
    }
}

fn action_wire(action: &BreakerAction) -> &'static str {
    match action {
        BreakerAction::HaltNewRisk => "HALT_NEW_RISK",
        BreakerAction::DisableVenue => "DISABLE_VENUE",
        BreakerAction::ReadOnly => "READ_ONLY",
        BreakerAction::Flatten => "FLATTEN",
    }
}

/// Apply the documented safety precedence.
#[must_use]
pub fn effective_action(breakers: &[CircuitBreakerDto]) -> Option<BreakerAction> {
    [
        BreakerAction::ReadOnly,
        BreakerAction::Flatten,
        BreakerAction::DisableVenue,
        BreakerAction::HaltNewRisk,
    ]
    .into_iter()
    .find(|action| {
        breakers
            .iter()
            .any(|breaker| breaker.active && breaker.action == *action)
    })
}

/// Append-only event used by the worker for deduplicated alerts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxEventDto {
    /// Event id.
    pub id: String,
    /// Stable event type.
    pub event_type: String,
    /// Aggregate identifier.
    pub aggregate_id: String,
    /// Structured event body.
    pub payload: serde_json::Value,
    /// RFC 3339 creation time.
    pub created_at: String,
}

/// Minimal durable outbox adapter.
#[derive(Clone)]
pub struct OutboxRepo {
    pool: PgPool,
}

impl OutboxRepo {
    /// Construct over a pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Append an event for later worker delivery.
    pub async fn append(
        &self,
        event_type: &str,
        aggregate_id: &str,
        payload: serde_json::Value,
        now: DateTime<Utc>,
    ) -> Result<OutboxEventDto, sqlx::Error> {
        let event = OutboxEventDto {
            id: Uuid::new_v4().to_string(),
            event_type: event_type.to_owned(),
            aggregate_id: aggregate_id.to_owned(),
            payload,
            created_at: now.to_rfc3339(),
        };
        let encoded = serde_json::to_string(&event)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        sqlx::query(
            "INSERT INTO outbox_events \
             (id, event_type, aggregate_id, payload, available_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $5)",
        )
        .bind(Uuid::parse_str(&event.id).map_err(|error| sqlx::Error::Protocol(error.to_string()))?)
        .bind(&event.event_type)
        .bind(&event.aggregate_id)
        .bind(encoded)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(event)
    }
}
