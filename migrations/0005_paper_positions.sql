-- M2 slice 3: paper positions. Each opened position is stored as a serialized
-- PositionDto payload (text) so the schema stays stable as the contract grows;
-- id/route_id/state/opened_at are kept as columns for lookup and ordering.
-- idempotency_key is unique so a repeated open with the same key resolves to the
-- existing row rather than creating a duplicate position. Paper-only, no live
-- trading. Slice 3 had no prior production data.

CREATE TABLE positions (
    id              uuid PRIMARY KEY,
    route_id        text NOT NULL,
    state           text NOT NULL,
    opened_at       timestamptz NOT NULL,
    idempotency_key text NOT NULL UNIQUE,
    payload         text NOT NULL
);

CREATE INDEX positions_opened_at_idx ON positions (opened_at DESC, id DESC);
