CREATE TABLE connector_certification_evidence (
    id UUID PRIMARY KEY,
    venue TEXT NOT NULL,
    venue_account_id UUID REFERENCES venue_accounts(id),
    product TEXT NOT NULL,
    environment TEXT NOT NULL,
    probe_kind TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('PASS', 'FAIL')),
    started_at TIMESTAMPTZ NOT NULL,
    completed_at TIMESTAMPTZ NOT NULL,
    payload TEXT NOT NULL
);

CREATE INDEX connector_certification_evidence_route_idx
    ON connector_certification_evidence
    (venue, venue_account_id, product, environment, completed_at DESC);

