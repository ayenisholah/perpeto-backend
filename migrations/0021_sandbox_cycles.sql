-- M4: one row per sandbox run on a testnet or demo venue. A CYCLE is a hedged
-- spot BUY + perpetual SELL opened, held through a funding settlement, and
-- closed. A RECOVERY is one deliberately ambiguous perpetual order resolved by
-- query before retry. Orders themselves stay in connector_order_intents; this
-- table ties a run's client order ids together and keeps the account
-- snapshots taken before and after it.
CREATE TABLE sandbox_cycles (
    id UUID PRIMARY KEY,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('CYCLE', 'RECOVERY')),
    venue TEXT NOT NULL,
    environment TEXT NOT NULL CHECK (environment IN ('TESTNET', 'DEMO')),
    underlying TEXT NOT NULL,
    spot_symbol TEXT,
    perp_symbol TEXT NOT NULL,
    spot_account_id TEXT,
    perp_account_id TEXT NOT NULL,
    base_quantity NUMERIC NOT NULL CHECK (base_quantity > 0),
    spot_open_client_id TEXT,
    perp_open_client_id TEXT NOT NULL,
    spot_close_client_id TEXT,
    perp_close_client_id TEXT,
    spot_filled NUMERIC NOT NULL DEFAULT 0,
    perp_filled NUMERIC NOT NULL DEFAULT 0,
    spot_closed NUMERIC NOT NULL DEFAULT 0,
    perp_closed NUMERIC NOT NULL DEFAULT 0,
    state TEXT NOT NULL CHECK (state IN ('OPENING', 'OPEN', 'CLOSING', 'CLOSED', 'FAILED')),
    funding_at TIMESTAMPTZ,
    snapshot_before JSONB,
    snapshot_after JSONB,
    outcome JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    opened_at TIMESTAMPTZ,
    closed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- At most one unfinished cycle per venue and tenant: the driver resumes it
-- instead of opening a second hedge.
CREATE UNIQUE INDEX sandbox_cycles_one_active
    ON sandbox_cycles (tenant_id, venue)
    WHERE kind = 'CYCLE' AND state IN ('OPENING', 'OPEN', 'CLOSING');

CREATE INDEX sandbox_cycles_venue_created ON sandbox_cycles (tenant_id, venue, created_at);

ALTER TABLE sandbox_cycles ENABLE ROW LEVEL SECURITY;
ALTER TABLE sandbox_cycles FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON sandbox_cycles
    USING (tenant_id = app_current_tenant_id())
    WITH CHECK (tenant_id = app_current_tenant_id());
