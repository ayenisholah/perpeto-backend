# Backend Implementation Plan

## Current objective

Restore a verified backend build, close the reopened M2 exact-decimal exit gate, and then finish M3 connector certification without enabling production writes. M4 follows as small, independently verifiable security and pilot slices. The current AES master-key/CLI credential path is transitional and cannot be reused as the M4 production design.

## Baseline recovery

1. Verify the local connector borrow/equality repair with locked Rust CI; do not treat the code as build-verified until compile, Clippy, migrations and tests pass together.
2. Replace every monetary, rate, price, quantity and PnL `f64` wire/persistence field with a lossless decimal representation, version the contract and update the vendored mobile client in lockstep.
3. Re-run the M1/M2 integration and evidence matrix before closing either milestone.

## M3 completion

1. Complete primary public/private WebSocket streams and connector-specific sequence recovery for Bybit, Binance and OKX.
2. Run credentialed sandbox/demo probes, reconciliation and timed soak evidence with least-privilege operator test accounts.
3. Run production read-only shadow evidence and record platform connector/route certification. Do not submit a production order.

## M4 ordered slices

1. **Tenant foundation:** add personal tenants and tenant membership; derive tenant context from authenticated sessions; tenant-scope IDs, commands, audit and idempotency.
2. **Database isolation:** add `tenant_id` to all private aggregates, tenant-aware foreign/unique constraints, forced RLS policies, non-bypass roles and tenant-bound repository transactions. Backfill existing paper data into an explicit migration tenant.
3. **Tenant-aware runtime:** scope engine jobs, reconciliation, worker queues/outbox, Redis keys/locks, WebSocket channels, breakers, alerts, exports and observability. Separate global platform state from tenant state.
4. **Passkeys:** add WebAuthn registration/assertion, RP/origin validation, replay protection, device/recovery lifecycle and one-use proofs bound to exact credential/live actions.
5. **Vault foundation:** deploy Vault Transit, create opaque per-tenant keys, workload auth/policies, 2-of-3 Shamir initialization, sealed readiness and audited operator runbooks.
6. **Credential lifecycle:** replace routine CLI enrollment with one-use API enrollment, encrypt-only API access, purpose-limited runtime decrypt, preflight validation, rotation, revocation, deletion, redaction and memory-lifetime controls.
7. **Account live gate:** separate platform connector certification from per-account permission/account/reconciliation/shadow preflight; persist eligibility and require passkey plus typed confirmation to arm.
8. **Limited CEX execution:** grant `PILOT` to one certified BTC/ETH route for a controlled cohort, enforce tenant/global breakers, loss/drawdown containment, disarm/flatten/revocation ordering and venue-authoritative recovery.
9. **Mobile/API contract evidence:** release versioned additive contracts, generated client and mobile flows only as each backend security boundary becomes real.
10. **Acceptance:** run cross-tenant/adversarial tests, Vault policy/seal/restore exercises, lost-device/recovery tests and a limited-capital pilot. M4 closes only with signed evidence.

## Change protocol

Each slice updates migrations/domain → service/repository → API/OpenAPI/client → mobile → tests/docs. Avoid a period where a secret endpoint exists without forced tenant isolation, passkey authorization and encrypt-only Vault access. Production write mode stays fail-closed until the complete gate is deployed and accepted.

## Explicitly deferred

DEX connectors/signers move to M5. Vault HA/managed auto-unseal, static egress improvements, stronger workload/host isolation, full DR/SLO/security review move to M6. `CERTIFIED_LIVE` and broader rollout move to M7. Automatic CEX withdrawals/transfers, team tenants, custody and double-entry ledger remain separately deferred; lossless-decimal persistence is an open M2 exit gate, not deferred live work.
