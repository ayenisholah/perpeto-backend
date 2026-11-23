-- M2 (DEC-0016): persist the market snapshot behind each scan.
--
-- The engine stores the exact legs, books and per-venue feed health it ranked,
-- so the API opens and the engine supervises paper positions against the same
-- data rather than reloading fixtures. The payload is a serialized
-- MarketSnapshotDto with decimals as strings, so it is lossless. It is public
-- market data only: no account, credential or tenant data, so scan_runs stays
-- platform-scoped. Rows written before this migration have no snapshot.
--
-- Staleness is judged on scan_runs.created_at, the wall-clock write time;
-- scanned_at is simulated time in fixture mode.

ALTER TABLE scan_runs ADD COLUMN market_snapshot text;

CREATE INDEX scan_runs_created_at_idx ON scan_runs (created_at DESC, id DESC)
    WHERE market_snapshot IS NOT NULL;
