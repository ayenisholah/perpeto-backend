-- M3 connector accounts, product profiles, exact instruments, fees, and health.

CREATE TABLE venue_accounts (
    id uuid PRIMARY KEY,
    venue text NOT NULL,
    alias text NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    certification_state text NOT NULL DEFAULT 'PAPER_ONLY',
    reconciliation_state text NOT NULL DEFAULT 'PENDING',
    masked_identity text NOT NULL,
    last_reconciled_at timestamptz,
    created_at timestamptz NOT NULL,
    tombstoned_at timestamptz,
    payload text NOT NULL,
    UNIQUE (venue, alias)
);

CREATE TABLE venue_product_connections (
    id uuid PRIMARY KEY,
    venue_account_id uuid NOT NULL REFERENCES venue_accounts(id),
    product text NOT NULL,
    environment text NOT NULL,
    secret_ref text,
    enabled boolean NOT NULL DEFAULT true,
    payload text NOT NULL,
    UNIQUE (venue_account_id, product, environment)
);

CREATE TABLE instruments (
    venue text NOT NULL,
    symbol text NOT NULL,
    product text NOT NULL,
    canonical_underlying text NOT NULL,
    quote_asset text NOT NULL,
    settlement_asset text NOT NULL,
    margin_asset text NOT NULL,
    contract_multiplier numeric NOT NULL,
    price_tick numeric NOT NULL,
    quantity_step numeric NOT NULL,
    minimum_quantity numeric NOT NULL,
    maximum_quantity numeric,
    minimum_notional numeric NOT NULL,
    funding_interval_hours integer,
    funding_cap numeric,
    funding_floor numeric,
    maximum_leverage numeric,
    trading boolean NOT NULL,
    metadata_version text NOT NULL,
    venue_updated_at timestamptz,
    received_at timestamptz NOT NULL,
    payload text NOT NULL,
    PRIMARY KEY (venue, symbol, product)
);

CREATE TABLE instrument_mappings (
    id uuid PRIMARY KEY,
    venue text NOT NULL,
    symbol text NOT NULL,
    product text NOT NULL,
    canonical_underlying text NOT NULL,
    approved_by uuid,
    approved_at timestamptz,
    haircut numeric NOT NULL DEFAULT 0,
    payload text NOT NULL,
    FOREIGN KEY (venue, symbol, product) REFERENCES instruments(venue, symbol, product)
);

CREATE TABLE fee_schedules (
    id uuid PRIMARY KEY,
    venue_account_id uuid NOT NULL REFERENCES venue_accounts(id),
    product text NOT NULL,
    maker numeric NOT NULL,
    taker numeric NOT NULL,
    effective_at timestamptz NOT NULL,
    expires_at timestamptz,
    payload text NOT NULL
);

CREATE TABLE venue_health_samples (
    id uuid PRIMARY KEY,
    venue_account_id uuid REFERENCES venue_accounts(id),
    venue text NOT NULL,
    environment text NOT NULL,
    status text NOT NULL,
    clock_drift_ms bigint NOT NULL,
    rate_limit_remaining bigint,
    observed_at timestamptz NOT NULL,
    payload text NOT NULL
);

CREATE INDEX venue_accounts_active_idx ON venue_accounts(venue, created_at) WHERE tombstoned_at IS NULL;
CREATE INDEX venue_health_latest_idx ON venue_health_samples(venue, observed_at DESC);
