-- M2 Slice 6: immutable versioned paper strategies and position attribution.

CREATE TABLE strategies (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    enabled boolean NOT NULL DEFAULT false,
    current_version integer NOT NULL,
    created_at timestamptz NOT NULL,
    payload text NOT NULL
);

CREATE TABLE strategy_versions (
    strategy_id uuid NOT NULL REFERENCES strategies(id) ON DELETE CASCADE,
    version integer NOT NULL,
    created_at timestamptz NOT NULL,
    payload text NOT NULL,
    PRIMARY KEY (strategy_id, version)
);

ALTER TABLE positions ADD COLUMN strategy_id uuid REFERENCES strategies(id);
CREATE INDEX positions_strategy_idx ON positions(strategy_id, opened_at DESC);
