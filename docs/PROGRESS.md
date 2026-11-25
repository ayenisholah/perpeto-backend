# Backend Progress

## Snapshot

- **Roadmap:** re-baselined by DEC-0015 on 2026-11-22. Perpeto is a public reference implementation, and LIVE is permanently fail-closed.
- **Milestones:** M1, M2 and M3 are done (M2 and M3 closed 2026-11-25). M4, testnet execution, is next.
- **Staging:** AWS `eu-north-1` (DEC-0020), tracking `main`. The API is at `https://api.perpeto.xyz`, and the live demo at `https://app.perpeto.xyz` (Next.js `perpeto-web`, DEC-0021).
- **CI:** Backend CI, container and staging deploy are green on `main`.
- **Last full local verification:** 2026-11-24: 200 tests passed, plus the fresh-clone `docker compose up`.

## Verification baseline

Established on Linux with the pinned Rust 1.97.0 toolchain and a PostgreSQL 18 container (`deploy/compose/docker-compose.dev.yml`). Building requires `pkg-config` and `libssl-dev`, because `webauthn-rs` links native OpenSSL. Reproduce with:

```bash
docker compose -f deploy/compose/docker-compose.dev.yml up -d
set -a; . ./deploy/dev.env; set +a
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
DATABASE_URL="$MIGRATE_DATABASE_URL" cargo run --locked --bin funding-arb-migrate
cargo test --locked --workspace
```

All 18 migrations apply and 136 tests pass with 0 failures. All eleven database-backed integration binaries execute rather than skipping:
- `audit_chain`
- `auth_flow`
- `forced_rls`
- `passkey_isolation`
- `pilot_state`
- `breakers_isolation`
- `positions_repo`
- `credential_deletion`
- `credential_enrollment`
- `credential_rewrap`
- `preflight_worker`

The isolation evidence is non-vacuous: run as the superuser instead of `perpeto_app`, `forced_rls` fails.

The first execution of this baseline exposed several defects, all now fixed:
- Migration `0016` named constraints beyond PostgreSQL's 63-byte limit.
- `VenueAccountsRepo::update_connection` never wrote `secret_ref`. That made deletion skip revocation and made `ELIGIBLE` unreachable.
- `ConnectorRegistry::load` shadowed its tenant context.
- The engine used a stale `ExecutionService::open` signature.

## M1 — Foundations (done)

- **Platform:** Rust workspace, PostgreSQL migrations, OpenAPI and event contracts, CI, container, staging, package release and cross-repository documentation automation.
- **Authentication:** Google and Apple social auth with rotating sessions, TOTP and recovery codes, plus the mobile authentication surfaces.
- **Paper trading:** deterministic paper scanner, the opportunity and risk model, and the coordinated open, close, re-hedge and recovery lifecycle. Also circuit breakers, strategies, portfolio and PnL, health, alerts, and the matching mobile monitoring.
- **Connectors:** Bybit, Binance and OKX REST adapters, primary public and private WebSocket streams with sequence recovery, exact-decimal books, the durable sandbox execution coordinator, reconciliation and accounting, and the evidence CLI.
- **Contracts:** an exact-decimal wire and persistence contract through API client `0.22.0`, which mobile vendors.

## Implemented hardening (kept, not a gate)

- **Isolation:** personal tenants, session-derived tenant context and forced RLS on private aggregates. Tenant-bound repository transactions, tenant-dispatched background work, and rejection of unsafe runtime roles.
- **Passkeys:** WebAuthn registration and assertion, with one-use action proofs bound to an exact `ActionDescriptor`.
- **Credentials:**
  - `VaultTransitSecretStore` (optional) beside the transitional AES store, plus a ciphertext-rewrap worker.
  - Passkey-gated enroll, rotate, revoke and delete.
  - A preflight endpoint and a scheduled preflight worker that demotes drifted routes.
- **Live gate:** `PilotState` arming and `permit_live_risk`, with disarm escalating to flatten. These are inert by design.
- **Audit:** a hash-chained audit log and an owner-only `GET /audit` with an integrity flag.

## Open items by milestone

- **M2 (done):** the evidence is in `docs/evidence/m2/`.
- **M3 (done):** the evidence is in `docs/evidence/m3/PUBLIC_DEMO.md`. Deferred by choice:
  - web Google/Apple sign-in, until the Google JavaScript origin and an Apple Services ID are configured;
  - TestFlight, while EAS is paused.
- **M4:** no testnet or demo credentials are enrolled and no execution evidence is committed.

## Next tasks

1. M4: enroll the operator's Bybit testnet, Binance testnet and OKX demo credentials through the SANDBOX coordinator, and record one open → funding → close cycle per venue.
2. M4: force one partial-fill or timeout recovery per venue and commit the reconciliation report under `docs/evidence/m4/`.
3. Configure web provider sign-in when wanted: the Google JavaScript origin `https://app.perpeto.xyz`, and an Apple Services ID in `PERPETO_APPLE_WEB_SERVICES_ID`.
