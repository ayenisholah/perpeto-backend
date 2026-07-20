CREATE TABLE connector_balance_snapshots (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES connector_reconciliation_runs(id),
    venue TEXT NOT NULL,
    venue_account_id TEXT NOT NULL,
    asset TEXT NOT NULL,
    total NUMERIC NOT NULL,
    available NUMERIC NOT NULL,
    locked NUMERIC NOT NULL,
    venue_updated_at TIMESTAMPTZ,
    received_at TIMESTAMPTZ NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE (run_id, asset)
);

CREATE TABLE connector_position_snapshots (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES connector_reconciliation_runs(id),
    venue TEXT NOT NULL,
    venue_account_id TEXT NOT NULL,
    symbol TEXT NOT NULL,
    product TEXT NOT NULL,
    quantity NUMERIC NOT NULL,
    entry_price NUMERIC NOT NULL,
    mark_price NUMERIC NOT NULL,
    leverage NUMERIC NOT NULL,
    liquidation_price NUMERIC,
    venue_updated_at TIMESTAMPTZ,
    received_at TIMESTAMPTZ NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE (run_id, symbol, product)
);

CREATE TABLE connector_funding_ledger (
    id UUID PRIMARY KEY,
    venue TEXT NOT NULL,
    venue_account_id TEXT NOT NULL,
    venue_entry_id TEXT NOT NULL,
    symbol TEXT NOT NULL,
    product TEXT NOT NULL,
    amount NUMERIC NOT NULL,
    asset TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    payload TEXT NOT NULL,
    UNIQUE (venue, venue_account_id, venue_entry_id)
);

CREATE INDEX connector_balance_latest_idx
    ON connector_balance_snapshots (venue, venue_account_id, received_at DESC);

CREATE INDEX connector_position_latest_idx
    ON connector_position_snapshots (venue, venue_account_id, received_at DESC);
