CREATE TABLE service_heartbeats (
    service text NOT NULL,
    status text NOT NULL,
    observed_at timestamptz NOT NULL,
    payload text NOT NULL,
    PRIMARY KEY (service, observed_at)
);
CREATE INDEX service_heartbeats_latest_idx ON service_heartbeats(service, observed_at DESC);
