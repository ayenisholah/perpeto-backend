CREATE TABLE notification_routes (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL,
    channel text NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    payload text NOT NULL
);
CREATE TABLE alerts (
    id uuid PRIMARY KEY,
    dedup_key text NOT NULL UNIQUE,
    severity text NOT NULL,
    created_at timestamptz NOT NULL,
    acknowledged_at timestamptz,
    acknowledged_by uuid,
    payload text NOT NULL
);
CREATE INDEX alerts_created_idx ON alerts(created_at DESC);
