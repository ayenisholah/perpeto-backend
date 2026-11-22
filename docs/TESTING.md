# Testing Strategy

Testing follows the release gates in `PRODUCT_SPEC.md`.

- Unit and property tests prove funding signs, decimal rounding, state transitions, risk precedence, and ledger balance.
- Connector contract tests replay redacted fixtures including gaps, duplicates, rejection, partial fill, timeout, rate limits, and venue-specific metadata.
- Integration tests use isolated PostgreSQL and Redis instances and deterministic clocks.
- Fault tests kill services at every lifecycle state and inject network, clock, disk, dependency, and ambiguous-order failures.
- Schema tests reject incompatible unversioned API or event changes.
- Authentication unit tests cover pending approval, privileged MFA gating, one-use bootstrap/challenges, fresh explicit provider linking, final-provider/final-Owner protection, device binding, refresh rotation/reuse, and deletion state. Runtime integration tests must additionally reject invalid provider signature/issuer/audience/nonce/expiry/code replay, concurrent refresh, replayed TOTP/recovery codes, and unauthorized pending-resource reads.
- Staging smoke tests require paper mode, blocked live entry, health endpoints, and synthetic notifications.

## Milestone evidence

- **M1 (done):** fmt, Clippy with warnings denied, all migrations, and the full suite (136 tests). The 11 database-backed suites run under the non-`BYPASSRLS` runtime role, and run as a superuser `forced_rls` fails, so the evidence binds.
- **M2:** public-feed parser tests against recorded public payloads; a stale-feed test proving entries are blocked and never filled from fixtures; a test proving PAPER constructs no credential read path; a 24-hour staging run with no crash.
- **M3:** a demo-viewer authorization test that calls every mutating endpoint and expects rejection; a smoke test from the hosted web build; a fresh-clone `docker compose up` check.
- **M4:** per venue, a SANDBOX open → funding → close cycle and a forced partial-fill or timeout recovery, reconciled. Evidence is committed under `docs/evidence/` per `CONNECTOR_EVIDENCE.md`.

## Implemented hardening suites

The multi-tenant and secret-management layer is implemented hardening (DEC-0015). Its suites must keep passing, but extending them is not a milestone gate. What they cover today:

- Forced RLS on every tenant table, with missing tenant context failing closed (`forced_rls`).
- Adversarial cross-tenant tests for positions, credentials, passkeys, tenant state, breakers and venue accounts.
- Passkey isolation, credential enrollment, rotation, revocation, deletion and rewrap, the preflight worker, pilot state and the audit chain.

The following stay design record, required only for a real-money deployment, which is a non-goal:
- Vault policy, seal, unseal and restore exercises;
- passkey lost-device and recovery tests;
- limited-pilot tests.

Skipped or quarantined tests cannot satisfy milestone evidence.
