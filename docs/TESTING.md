# Testing Strategy

Testing follows the release gates in `PRODUCT_SPEC.md`.

- Unit and property tests prove funding signs, decimal rounding, state transitions, risk precedence, and ledger balance.
- Connector contract tests replay redacted fixtures including gaps, duplicates, rejection, partial fill, timeout, rate limits, and venue-specific metadata.
- Integration tests use isolated PostgreSQL and Redis instances and deterministic clocks.
- Fault tests kill services at every lifecycle state and inject network, clock, disk, dependency, and ambiguous-order failures.
- Schema tests reject incompatible unversioned API or event changes.
- Authentication unit tests cover pending approval, privileged MFA gating, one-use bootstrap/challenges, fresh explicit provider linking, final-provider/final-Owner protection, device binding, refresh rotation/reuse, and deletion state. Runtime integration tests must additionally reject invalid provider signature/issuer/audience/nonce/expiry/code replay, concurrent refresh, replayed TOTP/recovery codes, and unauthorized pending-resource reads before M1A can close.
- Staging smoke tests require paper mode, blocked live entry, reconciliation, health endpoints, and synthetic notifications.

Skipped or quarantined tests cannot satisfy milestone evidence.
