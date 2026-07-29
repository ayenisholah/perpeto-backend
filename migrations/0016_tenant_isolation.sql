-- M4 isolation slice: private aggregates become tenant-owned and forced-RLS.
-- Legacy shared rows are preserved in a suspended tenant with no membership.

INSERT INTO tenants (id, display_name, state)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'Legacy migration quarantine',
    'SUSPENDED'
);

ALTER TABLE positions ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE circuit_breakers ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE outbox_events ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE strategies ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE strategy_versions ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE notification_routes ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE alerts ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE venue_accounts ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE venue_product_connections ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE fee_schedules ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE venue_health_samples ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE encrypted_credentials ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE connector_order_intents ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE connector_order_events ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE connector_fills ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE connector_reconciliation_runs ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE connector_discrepancies ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE connector_balance_snapshots ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE connector_position_snapshots ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE connector_funding_ledger ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;
ALTER TABLE audit_events ADD COLUMN tenant_id UUID REFERENCES tenants(id) ON DELETE RESTRICT;

UPDATE positions SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE circuit_breakers SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE outbox_events SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE strategies SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE strategy_versions SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE notification_routes SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE alerts SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE venue_accounts SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE venue_product_connections SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE fee_schedules SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE venue_health_samples
SET tenant_id = '00000000-0000-0000-0000-000000000001'
WHERE venue_account_id IS NOT NULL;
UPDATE encrypted_credentials SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE connector_order_intents SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE connector_order_events SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE connector_fills SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE connector_reconciliation_runs SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE connector_discrepancies SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE connector_balance_snapshots SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE connector_position_snapshots SET tenant_id = '00000000-0000-0000-0000-000000000001';
UPDATE connector_funding_ledger SET tenant_id = '00000000-0000-0000-0000-000000000001';

UPDATE audit_events event
SET tenant_id = session.tenant_id
FROM sessions session
WHERE event.session_id = session.id;

UPDATE audit_events event
SET tenant_id = membership.tenant_id
FROM audit_actors actor
JOIN tenant_memberships membership ON membership.user_id = actor.user_id
WHERE event.actor_id = actor.id AND event.tenant_id IS NULL;

ALTER TABLE positions ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE circuit_breakers ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE outbox_events ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE strategies ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE strategy_versions ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE notification_routes ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE alerts ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE venue_accounts ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE venue_product_connections ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE fee_schedules ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE encrypted_credentials ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE connector_order_intents ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE connector_order_events ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE connector_fills ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE connector_reconciliation_runs ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE connector_discrepancies ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE connector_balance_snapshots ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE connector_position_snapshots ALTER COLUMN tenant_id SET NOT NULL;
ALTER TABLE connector_funding_ledger ALTER COLUMN tenant_id SET NOT NULL;

-- These unique constraints were created inline by earlier migrations, so their
-- names are PostgreSQL-generated. Three of them exceed the 63-byte identifier
-- limit and are truncated: PostgreSQL shortens the column portion while keeping
-- the trailing `_key`, so the stored name is not simply the first 63 characters
-- of the obvious name. Each name below is the one actually in `pg_constraint`.
-- Do not add `IF EXISTS` here: a silently skipped drop would leave the old
-- tenant-blind unique constraint enforcing global uniqueness across tenants,
-- which is exactly the isolation defect this migration removes.
ALTER TABLE positions DROP CONSTRAINT positions_idempotency_key_key;
ALTER TABLE venue_accounts DROP CONSTRAINT venue_accounts_venue_alias_key;
ALTER TABLE venue_product_connections DROP CONSTRAINT venue_product_connections_venue_account_id_product_environm_key;
ALTER TABLE encrypted_credentials DROP CONSTRAINT encrypted_credentials_venue_account_id_product_key;
ALTER TABLE connector_order_intents DROP CONSTRAINT connector_order_intents_venue_venue_account_id_client_order_key;
ALTER TABLE connector_fills DROP CONSTRAINT connector_fills_venue_venue_account_id_venue_fill_id_key;
ALTER TABLE connector_funding_ledger DROP CONSTRAINT connector_funding_ledger_venue_venue_account_id_venue_entry_key;
ALTER TABLE alerts DROP CONSTRAINT alerts_dedup_key_key;

ALTER TABLE positions ADD CONSTRAINT positions_tenant_idempotency_key UNIQUE (tenant_id, idempotency_key);
ALTER TABLE venue_accounts ADD CONSTRAINT venue_accounts_tenant_venue_alias_key UNIQUE (tenant_id, venue, alias);
ALTER TABLE venue_accounts ADD CONSTRAINT venue_accounts_tenant_id_key UNIQUE (tenant_id, id);
ALTER TABLE venue_product_connections ADD CONSTRAINT venue_product_connections_tenant_key UNIQUE (tenant_id, venue_account_id, product, environment);
ALTER TABLE strategies ADD CONSTRAINT strategies_tenant_id_key UNIQUE (tenant_id, id);
ALTER TABLE connector_reconciliation_runs ADD CONSTRAINT connector_reconciliation_runs_tenant_id_key UNIQUE (tenant_id, id);
ALTER TABLE connector_order_intents ADD CONSTRAINT connector_order_intents_tenant_id_key UNIQUE (tenant_id, id);
ALTER TABLE connector_order_intents ADD CONSTRAINT connector_order_intents_tenant_client_key UNIQUE (tenant_id, venue, venue_account_id, client_order_id);
ALTER TABLE connector_fills ADD CONSTRAINT connector_fills_tenant_venue_fill_key UNIQUE (tenant_id, venue, venue_account_id, venue_fill_id);
ALTER TABLE connector_funding_ledger ADD CONSTRAINT connector_funding_ledger_tenant_entry_key UNIQUE (tenant_id, venue, venue_account_id, venue_entry_id);
ALTER TABLE alerts ADD CONSTRAINT alerts_tenant_dedup_key UNIQUE (tenant_id, dedup_key);

ALTER TABLE strategy_versions
    ADD CONSTRAINT strategy_versions_tenant_strategy_fk
    FOREIGN KEY (tenant_id, strategy_id) REFERENCES strategies(tenant_id, id) ON DELETE CASCADE;
ALTER TABLE positions
    ADD CONSTRAINT positions_tenant_strategy_fk
    FOREIGN KEY (tenant_id, strategy_id) REFERENCES strategies(tenant_id, id);
ALTER TABLE venue_product_connections
    ADD CONSTRAINT venue_product_connections_tenant_account_fk
    FOREIGN KEY (tenant_id, venue_account_id) REFERENCES venue_accounts(tenant_id, id);
ALTER TABLE fee_schedules
    ADD CONSTRAINT fee_schedules_tenant_account_fk
    FOREIGN KEY (tenant_id, venue_account_id) REFERENCES venue_accounts(tenant_id, id);
ALTER TABLE encrypted_credentials
    ADD CONSTRAINT encrypted_credentials_tenant_account_fk
    FOREIGN KEY (tenant_id, venue_account_id) REFERENCES venue_accounts(tenant_id, id) ON DELETE CASCADE;
ALTER TABLE connector_order_events
    ADD CONSTRAINT connector_order_events_tenant_intent_fk
    FOREIGN KEY (tenant_id, intent_id) REFERENCES connector_order_intents(tenant_id, id);
ALTER TABLE connector_discrepancies
    ADD CONSTRAINT connector_discrepancies_tenant_run_fk
    FOREIGN KEY (tenant_id, run_id) REFERENCES connector_reconciliation_runs(tenant_id, id);
ALTER TABLE connector_balance_snapshots
    ADD CONSTRAINT connector_balance_snapshots_tenant_run_fk
    FOREIGN KEY (tenant_id, run_id) REFERENCES connector_reconciliation_runs(tenant_id, id);
ALTER TABLE connector_position_snapshots
    ADD CONSTRAINT connector_position_snapshots_tenant_run_fk
    FOREIGN KEY (tenant_id, run_id) REFERENCES connector_reconciliation_runs(tenant_id, id);

DROP INDEX circuit_breakers_active_scope_action_idx;
CREATE UNIQUE INDEX circuit_breakers_active_tenant_scope_action_idx
    ON circuit_breakers (tenant_id, scope, COALESCE(scope_key, ''), action)
    WHERE active;
CREATE INDEX positions_tenant_opened_idx ON positions (tenant_id, opened_at DESC, id DESC);
CREATE INDEX outbox_events_tenant_pending_idx
    ON outbox_events (tenant_id, available_at, created_at)
    WHERE completed_at IS NULL;
CREATE INDEX connector_reconciliation_tenant_latest_idx
    ON connector_reconciliation_runs (tenant_id, venue, venue_account_id, started_at DESC);

-- A missing transaction-local context evaluates to NULL and matches no row.
-- Public venue-health rows remain readable, but tenant roles cannot create or
-- mutate them through the tenant policy.
DO $tenant_rls$
DECLARE
    table_name TEXT;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'positions', 'circuit_breakers', 'outbox_events', 'strategies',
        'strategy_versions', 'notification_routes', 'alerts', 'venue_accounts',
        'venue_product_connections', 'fee_schedules', 'encrypted_credentials',
        'connector_order_intents', 'connector_order_events', 'connector_fills',
        'connector_reconciliation_runs', 'connector_discrepancies',
        'connector_balance_snapshots', 'connector_position_snapshots',
        'connector_funding_ledger'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', table_name);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', table_name);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I USING (tenant_id = app_current_tenant_id()) WITH CHECK (tenant_id = app_current_tenant_id())',
            table_name
        );
    END LOOP;
END
$tenant_rls$;

ALTER TABLE venue_health_samples ENABLE ROW LEVEL SECURITY;
ALTER TABLE venue_health_samples FORCE ROW LEVEL SECURITY;
CREATE POLICY venue_health_read ON venue_health_samples
    FOR SELECT USING (tenant_id IS NULL OR tenant_id = app_current_tenant_id());
CREATE POLICY venue_health_tenant_write ON venue_health_samples
    FOR ALL USING (tenant_id = app_current_tenant_id())
    WITH CHECK (tenant_id = app_current_tenant_id());

ALTER TABLE audit_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_events FORCE ROW LEVEL SECURITY;
CREATE POLICY audit_tenant_isolation ON audit_events
    USING (tenant_id = app_current_tenant_id())
    WITH CHECK (tenant_id = app_current_tenant_id());
