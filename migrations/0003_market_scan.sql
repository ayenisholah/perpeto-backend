-- M2 slice 1: deterministic funding-arbitrage opportunity scanner.
--
-- A scan_run is one execution of the scanner; opportunities are its ranked,
-- eligible two-leg routes. Numeric fields are double precision (display/paper
-- fidelity); the precise decimal math happens in funding_arb_domain before
-- persistence. Paper-only: no venue, credential, or execution data is stored.

CREATE TABLE scan_runs (
    id            uuid PRIMARY KEY,
    scanned_at    timestamptz NOT NULL,
    model_version text        NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX scan_runs_scanned_at_idx ON scan_runs (scanned_at DESC, id DESC);

CREATE TABLE opportunities (
    id                        bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    scan_run_id               uuid NOT NULL REFERENCES scan_runs (id) ON DELETE CASCADE,
    rank_position             integer NOT NULL,
    route_id                  text NOT NULL,
    route_type                text NOT NULL CHECK (route_type IN ('SPOT_PERP', 'PERP_PERP')),
    underlying                text NOT NULL,
    long_venue                text NOT NULL,
    short_venue               text NOT NULL,
    gross_funding_apr         double precision NOT NULL,
    entry_cost                double precision NOT NULL,
    exit_cost                 double precision NOT NULL,
    carry_cost                double precision NOT NULL,
    risk_buffer               double precision NOT NULL,
    net_apr                   double precision NOT NULL,
    expected_net_pnl          double precision NOT NULL,
    profitability_probability double precision NOT NULL,
    available_capacity        double precision NOT NULL,
    next_funding_at           timestamptz NOT NULL
);

CREATE INDEX opportunities_scan_rank_idx ON opportunities (scan_run_id, rank_position);
