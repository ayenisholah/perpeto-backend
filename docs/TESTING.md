# Testing Strategy

Testing follows the release gates in `PRODUCT_SPEC.md`.

- Unit and property tests prove funding signs, decimal rounding, state transitions, risk precedence, and ledger balance.
- Connector contract tests replay redacted fixtures including gaps, duplicates, rejection, partial fill, timeout, rate limits, and venue-specific metadata.
- Integration tests use isolated PostgreSQL and Redis instances and deterministic clocks.
- Fault tests kill services at every lifecycle state and inject network, clock, disk, dependency, and ambiguous-order failures.
- Schema tests reject incompatible unversioned API or event changes.
- Staging smoke tests require paper mode, blocked live entry, reconciliation, health endpoints, and synthetic notifications.

Skipped or quarantined tests cannot satisfy milestone evidence.
