# Backend Progress

## Snapshot

- **Roadmap:** re-baselined by DEC-0015 on 2026-11-22. Perpeto is a public reference implementation, and LIVE is permanently fail-closed.
- **Milestone:** M1 done. M2, live-data paper trading, is next.
- **CI:** Backend CI, container and staging deploy were green on `main` at `07cb3cc` (2026-07-30).
- **Staging:** `https://api.perpeto.xyz/livez` returned 502 from the host proxy on 2026-11-22. Restoring it is the first M3 task.
- **Last full local verification:** 2026-07-29.

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

- **M2:**
  - No PAPER path consumes live public data yet; the engine replays fixtures on simulated time.
  - `book_snapshot` is unwired on the Bybit and OKX reconnect paths.
  - The public probe (`funding-arb-connector-evidence public`) has not been recorded.
- **M3:** staging is down (502); no demo viewer; no hosted web build; no one-command local start; the API client registry (`0.7.0`) lags the vendored `0.22.0`.
- **M4:** no testnet or demo credentials are enrolled and no execution evidence is committed.

## Next tasks

1. M2: build the credential-free `PublicMarketSource` and wire it into the engine's PAPER path (`docs/IMPLEMENTATION.md`).
2. M2: simulate paper fills against live depth, and add stale-feed blocking with its tests.
3. M2: wire `book_snapshot` into the Bybit and OKX reconnect paths.
4. M3: restore staging and redeploy `main`.
