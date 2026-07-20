CREATE TABLE connector_order_intents (
    id UUID PRIMARY KEY,
    venue TEXT NOT NULL,
    venue_account_id TEXT NOT NULL,
    client_order_id TEXT NOT NULL,
    venue_order_id TEXT,
    symbol TEXT NOT NULL,
    product TEXT NOT NULL,
    side TEXT NOT NULL,
    base_quantity NUMERIC NOT NULL,
    limit_price NUMERIC NOT NULL,
    native_quantity NUMERIC NOT NULL,
    native_price NUMERIC NOT NULL,
    equivalent_base_quantity NUMERIC NOT NULL,
    projected_delta_usd NUMERIC NOT NULL,
    reduce_only BOOLEAN NOT NULL,
    environment TEXT NOT NULL,
    state TEXT NOT NULL,
    prepared_payload TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    error_code TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (venue, venue_account_id, client_order_id)
);

CREATE TABLE connector_order_events (
    id UUID PRIMARY KEY,
    intent_id UUID NOT NULL REFERENCES connector_order_intents(id),
    status TEXT NOT NULL,
    venue_order_id TEXT,
    venue_sequence BIGINT,
    venue_updated_at TIMESTAMPTZ,
    received_at TIMESTAMPTZ NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE (intent_id, status, venue_sequence, venue_updated_at)
);

CREATE TABLE connector_fills (
    id UUID PRIMARY KEY,
    venue TEXT NOT NULL,
    venue_account_id TEXT NOT NULL,
    venue_fill_id TEXT NOT NULL,
    venue_order_id TEXT NOT NULL,
    symbol TEXT NOT NULL,
    product TEXT NOT NULL,
    quantity NUMERIC NOT NULL,
    price NUMERIC NOT NULL,
    fee NUMERIC NOT NULL,
    fee_asset TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE (venue, venue_account_id, venue_fill_id)
);

CREATE TABLE connector_reconciliation_runs (
    id UUID PRIMARY KEY,
    venue TEXT NOT NULL,
    venue_account_id TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ,
    balances_seen INTEGER NOT NULL DEFAULT 0,
    positions_seen INTEGER NOT NULL DEFAULT 0,
    open_orders_seen INTEGER NOT NULL DEFAULT 0,
    fills_seen INTEGER NOT NULL DEFAULT 0,
    payload TEXT NOT NULL
);

CREATE TABLE connector_discrepancies (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES connector_reconciliation_runs(id),
    kind TEXT NOT NULL,
    severity TEXT NOT NULL,
    external_key TEXT NOT NULL,
    status TEXT NOT NULL,
    detected_at TIMESTAMPTZ NOT NULL,
    resolved_at TIMESTAMPTZ,
    payload TEXT NOT NULL,
    UNIQUE (run_id, kind, external_key)
);

CREATE INDEX connector_intents_unresolved_idx
    ON connector_order_intents (venue, venue_account_id, updated_at)
    WHERE state IN ('SUBMITTING', 'UNKNOWN_OUTCOME', 'PARTIALLY_FILLED');

CREATE INDEX connector_reconciliation_latest_idx
    ON connector_reconciliation_runs (venue, venue_account_id, started_at DESC);
