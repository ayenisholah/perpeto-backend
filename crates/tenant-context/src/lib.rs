//! Immutable tenant identity and transaction-local PostgreSQL enforcement.

use std::{fmt, str::FromStr};

use sqlx::{PgConnection, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TenantId(Uuid);

impl TenantId {
    #[must_use]
    pub const fn new(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl FromStr for TenantId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TenantContext {
    tenant_id: TenantId,
}

impl TenantContext {
    #[must_use]
    pub const fn new(tenant_id: TenantId) -> Self {
        Self { tenant_id }
    }

    #[must_use]
    pub const fn tenant_id(self) -> TenantId {
        self.tenant_id
    }
}

pub struct TenantTransaction<'a>(Transaction<'a, Postgres>);

impl<'a> TenantTransaction<'a> {
    pub async fn begin(pool: &'a PgPool, context: TenantContext) -> Result<Self, sqlx::Error> {
        let mut transaction = pool.begin().await?;
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(context.tenant_id().to_string())
            .execute(&mut *transaction)
            .await?;
        let selected: Option<Uuid> = sqlx::query_scalar("SELECT app_current_tenant_id()")
            .fetch_one(&mut *transaction)
            .await?;
        if selected != Some(context.tenant_id().as_uuid()) {
            return Err(sqlx::Error::Protocol(
                "transaction-local tenant context was not established".into(),
            ));
        }
        Ok(Self(transaction))
    }

    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.0.commit().await
    }

    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.0.rollback().await
    }
}

impl<'a> TenantTransaction<'a> {
    pub fn connection(&mut self) -> &mut PgConnection {
        &mut self.0
    }
}

/// Reject runtime roles that can bypass forced tenant policies or own tenant
/// tables. Migration binaries deliberately do not call this function.
pub async fn assert_safe_runtime_role(pool: &PgPool) -> Result<(), sqlx::Error> {
    let row = sqlx::query(
        "SELECT role.rolbypassrls,
                EXISTS (
                    SELECT 1 FROM pg_class table_class
                    JOIN pg_namespace namespace ON namespace.oid = table_class.relnamespace
                    WHERE table_class.relkind = 'r'
                      AND namespace.nspname = current_schema()
                      AND table_class.relowner = role.oid
                      AND table_class.relname IN (
                        'positions', 'strategies', 'venue_accounts',
                        'connector_order_intents', 'alerts', 'outbox_events'
                      )
                ) AS owns_tenant_table
         FROM pg_roles role WHERE role.rolname = current_user",
    )
    .fetch_one(pool)
    .await?;
    if row.get::<bool, _>("rolbypassrls") || row.get::<bool, _>("owns_tenant_table") {
        return Err(sqlx::Error::Protocol(
            "runtime database role may bypass or own tenant tables".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_ids_must_be_valid_uuids() {
        assert!(TenantId::from_str("tenant-a").is_err());
        let raw = Uuid::new_v4();
        assert_eq!(
            TenantId::from_str(&raw.to_string())
                .expect("valid")
                .as_uuid(),
            raw
        );
    }
}
