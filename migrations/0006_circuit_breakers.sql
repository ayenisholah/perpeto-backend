-- M2 slice 5b: durable paper circuit breakers and transactional event outbox.

CREATE TABLE circuit_breakers (
    id          uuid PRIMARY KEY,
    scope       text NOT NULL,
    scope_key   text,
    action      text NOT NULL,
    source      text NOT NULL,
    active      boolean NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL,
    resolved_at timestamptz,
    payload     text NOT NULL
);

CREATE UNIQUE INDEX circuit_breakers_active_scope_action_idx
    ON circuit_breakers (scope, COALESCE(scope_key, ''), action)
    WHERE active;
CREATE INDEX circuit_breakers_active_created_idx
    ON circuit_breakers (created_at DESC) WHERE active;

CREATE TABLE outbox_events (
    id           uuid PRIMARY KEY,
    event_type   text NOT NULL,
    aggregate_id text NOT NULL,
    payload      text NOT NULL,
    available_at timestamptz NOT NULL,
    claimed_at   timestamptz,
    completed_at timestamptz,
    attempts     integer NOT NULL DEFAULT 0,
    last_error   text,
    created_at   timestamptz NOT NULL
);

CREATE INDEX outbox_events_pending_idx
    ON outbox_events (available_at, created_at)
    WHERE completed_at IS NULL;
