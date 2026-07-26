# Backend Implementation Plan

## Current objective

Restore a verified backend build, obtain the Rust/PostgreSQL evidence needed to close the reopened M2 exact-decimal exit gate, and then finish M3 connector certification without enabling production writes. Continue M4 only as small, independently verifiable security and pilot slices; the personal-tenant session foundation is implemented locally, but isolation is not complete. The current AES master-key/CLI credential path is transitional and cannot be reused as the M4 production design.

## Baseline recovery

1. Verify the local connector borrow/equality repair with locked Rust CI; do not treat the code as build-verified until compile, Clippy, migrations and tests pass together.
2. **Implemented locally; evidence open:** replace every monetary, rate, price, quantity and PnL `f64` wire/persistence field with a lossless decimal representation, version the contract and update the vendored mobile client in lockstep.
3. Re-run the M1/M2 integration and evidence matrix before closing either milestone.

## M3 completion

1. **Implemented locally; build/live evidence open:** primary public/private WebSocket streams, bounded reconnect handling and connector-specific sequence recovery for Bybit, Binance and OKX.
2. Compile and run the transport/parser suites, then run credentialed sandbox/demo probes, reconciliation and timed soak evidence with least-privilege operator test accounts.
3. Run production read-only shadow evidence and record platform connector/route certification. Do not submit a production order.

## M4 ordered slices

1. **Tenant foundation — partially implemented locally:** personal tenants, one-owner membership, session-derived tenant context and public tenant summary are present. Tenant-scoped private IDs, commands, audit and idempotency remain part of the next isolation slices.
2. **Database isolation — implemented locally; evidence open:** migration `0016` adds tenant ownership, tenant-aware foreign/unique constraints and forced RLS, quarantines legacy rows in a suspended tenant, and repositories establish transaction-local tenant context. Runtime binaries reject owner or `BYPASSRLS` roles. Execute migration and adversarial tests under distinct roles before accepting this slice.
3. **Tenant-aware runtime — partially implemented locally:** API repositories, engine work, reconciliation, worker queues/outbox, breakers, alerts, connector accounting and encrypted credentials are tenant-scoped. Redis keys/locks, tenant WebSocket delivery, exports and full observability separation remain.
4. **Passkeys — implemented locally; build/DB evidence open:** WebAuthn registration/assertion, RP/origin validation, monotonic signature-count clone detection, single-use challenges and one-use action proofs bound to an exact `ActionDescriptor` are present (migration `0017`, `funding_arb_domain::auth`, `auth-service/src/passkey.rs`, additive API/OpenAPI `0.16.0` and a flagged mobile surface). Linked Rust compilation, the credentialed assertion round-trip, and the forced-RLS/adversarial passkey tests remain to run in a usable build environment. Device/recovery lifecycle beyond register/revoke and binding proofs to concrete credential/live actions follow with slices 6–8.
5. **Vault foundation — application code implemented locally; infra + evidence open:** a `VaultTransitSecretStore` (`crates/venue-service/src/vault.rs`) implements the existing `SecretStore` trait with per-tenant opaque Transit keys, record-binding `associated_data`, and separate encrypt-only (API) vs decrypt-only (workload) tokens; migration `0018` adds the `VAULT` credential row shape; runtime binaries select Vault when `PERPETO_VAULT_ADDR` is set, else the transitional AES store, fail-closed outside PAPER. Still to do (infra): deploy Vault Transit, create the per-tenant keys, workload auth/policies, 2-of-3 Shamir initialization, sealed readiness, the rewrap job and audited operator runbooks, plus the live encrypt→decrypt/policy evidence.
6. **Credential lifecycle — enroll/rotate/revoke implemented locally; live preflight + deletion open:** a passkey-gated API (`bins/api` `venue-credentials` routes → `CredentialEnrollmentService`, `crates/venue-service/src/enrollment.rs`) replaces the CLI: `enroll` creates the account and writes the secret through the encrypt-only `SecretStore` (Vault outside PAPER), `rotate`/`revoke` reuse it, and each op consumes a one-use passkey `ActionProof` bound to the exact action. Secrets are write-only and never returned or logged; endpoints 503 in PAPER. A `PENDING_PREFLIGHT → ELIGIBLE/PREFLIGHT_FAILED` state machine exists, but the worker that performs live permission/reconcile validation, credential deletion, and the ciphertext-rewrap job remain deferred.
7. **Account live gate:** separate platform connector certification from per-account permission/account/reconciliation/shadow preflight; persist eligibility and require passkey plus typed confirmation to arm.
8. **Limited CEX execution:** grant `PILOT` to one certified BTC/ETH route for a controlled cohort, enforce tenant/global breakers, loss/drawdown containment, disarm/flatten/revocation ordering and venue-authoritative recovery.
9. **Mobile/API contract evidence:** release versioned additive contracts, generated client and mobile flows only as each backend security boundary becomes real.
10. **Acceptance:** run cross-tenant/adversarial tests, Vault policy/seal/restore exercises, lost-device/recovery tests and a limited-capital pilot. M4 closes only with signed evidence.

## Change protocol

Each slice updates migrations/domain → service/repository → API/OpenAPI/client → mobile → tests/docs. Avoid a period where a secret endpoint exists without forced tenant isolation, passkey authorization and encrypt-only Vault access. Production write mode stays fail-closed until the complete gate is deployed and accepted.

## Explicitly deferred

DEX connectors/signers move to M5. Vault HA/managed auto-unseal, static egress improvements, stronger workload/host isolation, full DR/SLO/security review move to M6. `CERTIFIED_LIVE` and broader rollout move to M7. Automatic CEX withdrawals/transfers, team tenants, custody and double-entry ledger remain separately deferred; lossless-decimal persistence is an open M2 exit gate, not deferred live work.
