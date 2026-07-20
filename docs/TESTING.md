# Testing Strategy

Testing follows the release gates in `PRODUCT_SPEC.md`.

- Unit and property tests prove funding signs, decimal rounding, state transitions, risk precedence, and ledger balance.
- Connector contract tests replay redacted fixtures including gaps, duplicates, rejection, partial fill, timeout, rate limits, and venue-specific metadata.
- Integration tests use isolated PostgreSQL and Redis instances and deterministic clocks.
- Fault tests kill services at every lifecycle state and inject network, clock, disk, dependency, and ambiguous-order failures.
- Schema tests reject incompatible unversioned API or event changes.
- Authentication unit tests cover pending approval, privileged MFA gating, one-use bootstrap/challenges, fresh explicit provider linking, final-provider/final-Owner protection, device binding, refresh rotation/reuse, and deletion state. Runtime integration tests must additionally reject invalid provider signature/issuer/audience/nonce/expiry/code replay, concurrent refresh, replayed TOTP/recovery codes, and unauthorized pending-resource reads before M1A can close.
- Staging smoke tests require paper mode, blocked live entry, reconciliation, health endpoints, and synthetic notifications.

## M4 multi-user and secret-management gates

These tests describe the accepted target and must pass before any production CEX write:

- Cross-tenant API tests attempt every read, mutation, WebSocket subscription, export, alert acknowledgement and idempotency-key reuse with tenant A/B resources; all deny without disclosing existence.
- Repository tests run under non-owner database roles, prove forced RLS on every tenant table, prove missing context fails closed, and cover tenant-aware foreign keys, uniqueness, migrations, maintenance roles and connection-pool context reset.
- Background tests cover engine dispatch, reconciliation, outbox/jobs, Redis locks/cache keys, breakers and notifications so a tenantless or mismatched job cannot read or affect another tenant.
- WebAuthn tests cover RP ID/origin, challenge expiry, user verification, credential ownership, replay/counter handling, action/resource digest binding, final-passkey recovery and lost-device/session revocation.
- Vault policy tests prove API encrypt-only access, workload purpose-limited decrypt, denial across tenant key paths, denial of key administration, short-lease rotation and absence of root/unseal material from runtime.
- Credential tests cover one-use submission, ciphertext/context substitution, redaction, crash/log/trace/queue inspection, rotation cutover/rollback, revocation, tenant deletion and bounded plaintext lifetime.
- Operational tests cover 2-of-3 initialization/unseal, sealed startup, Vault outage, backup/restore without shares, alerting and safe behavior for existing versus new risk.
- Live-gate tests prove platform certification alone is insufficient; each account needs permission/account/reconciliation/shadow preflight and a fresh passkey/typed arm. Disarm, tenant breaker, global breaker and revocation block new risk immediately.
- Limited-pilot tests use dedicated low-balance accounts, withdrawal/transfer/admin-disabled keys, venue-authoritative reconciliation and explicit rollback evidence.

Skipped or quarantined tests cannot satisfy milestone evidence.
