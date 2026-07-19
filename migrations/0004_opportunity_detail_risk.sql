-- M2 slice 2: richer opportunities (detail fields, per-leg forecast, and risk
-- reservation) are stored as a serialized DTO payload so the schema stays stable
-- as the contract grows. route_id is kept as a column for direct detail lookup.
-- Slice 1 had no production data, so the table is recreated.

DROP TABLE IF EXISTS opportunities;

CREATE TABLE opportunities (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    scan_run_id   uuid NOT NULL REFERENCES scan_runs (id) ON DELETE CASCADE,
    rank_position integer NOT NULL,
    route_id      text NOT NULL,
    payload       text NOT NULL
);

CREATE INDEX opportunities_scan_rank_idx ON opportunities (scan_run_id, rank_position);
CREATE INDEX opportunities_scan_route_idx ON opportunities (scan_run_id, route_id);
