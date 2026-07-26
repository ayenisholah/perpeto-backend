# Backend Progress

## Snapshot

- Milestone: M3 — CEX connector certification
- Overall state: In progress (M2 feature implementation present but exit gates open; M3 code present through evidence tooling)
- Last verified: 2026-07-21

## Completed or implemented

- Established the Rust workspace, PostgreSQL migrations, OpenAPI/event contracts, CI, container, staging, package-release and cross-repository documentation automation.
- Implemented the M1A Google/Apple authentication domain, PostgreSQL repositories, runtime provider adapters, HTTP endpoints, rotating sessions, TOTP/recovery flows and mobile authentication surfaces. The last verified staging deployment contains this pre-M2 baseline.
- Implemented the M2 paper scanner, opportunity/risk model, coordinated open/close/re-hedge/recovery lifecycle, circuit breakers, strategies, portfolio/PnL, health, alerts and corresponding mobile monitoring surfaces.
- Added M3 Slices 1–21: fail-closed runtime modes, exact-decimal connector models/books, transitional encrypted administration, durable execution/reconciliation/accounting, Bybit/Binance/OKX REST and primary WebSocket adapters, sequence recovery, masked APIs, runtime wiring, observability and evidence tooling.
- Advanced the local OpenAPI/client contract through `@ayenisholah/perpeto-api-client@0.15.0`; the lossless-decimal contract introduced in `0.14.0` remains intact, local typecheck and 21 contract tests pass, and the mobile app consumes the matching `0.15.0` vendored tarball.
- Verified that documentation synchronization and mobile API-update automation are configured; `0.7.0` is the latest API-client version confirmed published to GitHub Packages.
- Added the first M4 isolation slices: personal tenants and session-derived context plus a quarantine-only legacy backfill, tenant columns/composite constraints, forced RLS on private trading aggregates, tenant-bound repository transactions, tenant-dispatched background work, and unsafe runtime-role rejection. No caller-selectable tenant input or secret endpoint was added.
- Added the M4 pilot-arming control plane locally: a pure `PilotState` machine (`crates/funding-arb-domain/src/pilot.rs`) and the first `tenants.state` write path (`auth-service` `pilot_state`/`set_pilot_state`). Owner-only `pilot` endpoints grant eligibility, arm a preflight-`ELIGIBLE` route with a one-use passkey `ActionProof` (`ArmPilot`) plus a typed alias confirmation, and disarm (always available; halts new risk). Transitions emit outbox events. Arming is control-plane only — LIVE stays fail-closed, so it changes no execution. OpenAPI/client `0.18.0` (vendored into mobile) and a flag-gated mobile Pilot card were added. Client typecheck + tests and mobile typecheck/lint/14 tests pass.
- Added the M4 credential-lifecycle API locally: passkey-gated enroll/rotate/revoke. New `ActionDescriptor` variants (`EnrollCredential`/`RotateCredential`/`RevokeCredential`) bind each one-use proof; `CredentialEnrollmentService` (`crates/venue-service/src/enrollment.rs`) creates the account and writes through the encrypt-only `SecretStore`, attaching only `secret_ref` and setting `PENDING_PREFLIGHT`. The `venue-credentials` API pairs (start/finish) verify the passkey proof before any secret operation, accept write-only credentials that are never logged or returned, and are 503 in PAPER. OpenAPI/client `0.17.0` (vendored into mobile) and a flag-gated mobile enrollment form were added. Client typecheck + 21 tests and mobile typecheck/lint/14 tests pass.
- Added the M4 Vault foundation code locally: a `VaultTransitSecretStore` (`crates/venue-service/src/vault.rs`) implementing the existing `SecretStore` trait against the Vault Transit HTTP API — per-tenant opaque keys, record-binding `associated_data`, and separate encrypt-only/decrypt-only tokens. Migration `0018` adds the `VAULT` credential row shape to the already forced-RLS `encrypted_credentials` table; `ConnectorRegistry::from_env`/`secret_store_from_env` select Vault when `PERPETO_VAULT_ADDR` is set and otherwise the transitional AES store, fail-closed outside PAPER, and all connector bins plus the `venue-admin` CLI route through it. No new API endpoint, contract or mobile change.
- Added the M4 passkey slice locally: WebAuthn registration/assertion and one-use action-authorization proofs. Migration `0017` adds `passkey_credentials`, `webauthn_challenges` and `passkey_action_proofs`, each forced-RLS under the same policy as `0016`; `funding_arb_domain::auth` owns the monotonic sign-count clone check and one-use `ActionProof`; `auth-service/src/passkey.rs` delegates verification to `webauthn-rs` and persists only through tenant transactions. Additive endpoints, OpenAPI/client `0.16.0` (vendored into mobile) and a flag-gated mobile passkey surface were added. The TypeScript client typechecks and its 21 contract tests plus the mobile typecheck/lint/tests pass; no secret-exposing endpoint was introduced.

## Exit gates and unverified work

- GitHub Backend CI for remote `main` at `6e59bad` is red: connector compilation reported 24 ownership/equality errors, so migrations and Rust tests did not run and the container/staging workflows were skipped. Local `main` contains the targeted borrow/equality repair, but it is not remotely compiled or CI-verified.
- The latest successful staging deployment is `e82347b`, which predates the M2/M3 implementation. Current staging therefore must not be described as containing the connector runtime.
- The M2 exact-decimal source/contract remediation is implemented locally: scanner, strategy, execution and monitoring DTOs use `Decimal`, OpenAPI/client `0.15.0` retains decimal strings, fixtures preserve exact numeric lexemes, and mobile performs numeric conversion only for display. The exit remains open until Rust and PostgreSQL round-trip tests run in a usable build environment.
- Primary public/private WebSocket ingestion and connector-specific sequence recovery are implemented locally for Bybit, Binance and OKX, including bounded reconnecting transport and disconnect-to-reconciliation signaling. Linked Rust compilation and live venue evidence remain pending.
- No venue has accepted certification evidence. Credentialed Bybit/Binance testnet, OKX demo, production read-only shadow, reconciliation and the required 72-hour/14-day windows remain outstanding.
- M1A real-device, accessibility, security and full provider/replay integration evidence remains incomplete.
- The first forced-RLS/private-runtime isolation slice is implemented locally. PostgreSQL execution under distinct migration/runtime roles, broader adversarial API/job evidence, tenant-scoped caches/WebSocket delivery/exports, Vault Transit, credential enrollment, account preflight and production CEX writes remain M4 work.
- The pilot-arming control plane is implemented locally but not build-verified. Linked Rust compilation, the `PilotState` unit tests, and the `DATABASE_URL`-gated `pilot_state` round-trip test remain. Arming has no live effect while LIVE is locked; wiring `PILOT_ARMED` into execution (Slice 8), platform route certification, and audit-chained arming records are deferred.
- The credential-lifecycle API is implemented locally but not build-verified. Linked Rust compilation, the AES-backed `credential_enrollment` integration test (enroll/rotate/revoke + cross-tenant read isolation, `DATABASE_URL`-gated), and the deferred live preflight worker/deletion remain. No credential is enrolled through a live Vault until the Vault infrastructure exists.
- The Vault Transit store is implemented locally but not build- or Vault-verified. Linked Rust compilation, the gated encrypt→decrypt round-trip, the encrypt-token-cannot-decrypt policy check, and the whole operator infrastructure (Vault deploy, per-tenant keys, Shamir, policies, rewrap, runbooks per `docs/KEY_MANAGEMENT.md`) remain. Paper/dev are unchanged (AES); the store is only reachable outside PAPER when `PERPETO_VAULT_ADDR` is configured.
- The passkey slice is implemented locally but not build- or database-verified. Linked Rust compilation of `webauthn-rs` and `auth-service`, a credentialed registration→assertion round-trip, and the new `passkey_isolation` forced-RLS/cross-tenant tests (which require a non-`BYPASSRLS` runtime role) remain to run. Passkey proofs are recorded but not yet consumed by any credential or live-arming gate, and the native mobile device ceremony stays dark behind `EXPO_PUBLIC_PASSKEYS_ENABLED` until an EAS development build exists.
- Live trading remains unavailable by design.

## Current blockers

- This Windows environment has Rust 1.97.0 but no usable MSVC linker, Docker or WSL, so linked Rust compile, Clippy and test evidence must remain pending until a suitable environment or a future CI run is available.
- Credentialed certification requires least-privilege operator test accounts and elapsed soak/shadow windows.
- API client `0.15.0` is locally verified but unpublished; `0.7.0` is the latest confirmed registry release.

## Next verified tasks

1. Restore a usable local Rust linker or use an approved verification environment; compile and execute the new WebSocket/parser and tenant-isolation tests before treating either slice as build-verified.
2. Run the implemented lossless-decimal DTO/persistence and precision-regression tests before closing M2.
3. Run the credentialed M3 sandbox/demo/shadow certification matrix and timed windows without production writes.
4. Verify migration `0016` with distinct migrator/runtime roles and adversarial cross-tenant database/API/job tests.
5. Compile the passkey slice (`webauthn-rs`, `auth-service`), run the passkey domain, isolation and assertion round-trip tests under a non-`BYPASSRLS` role.
6. Provision a dev Vault, run the `venue-service` unit tests and the `PERPETO_VAULT_ADDR`-gated encrypt→decrypt round-trip plus the encrypt-token-cannot-decrypt policy check.
7. Run the `credential_enrollment` and `pilot_state` integration tests, then build the deferred live preflight worker (permission inspection + reconciliation) and Slice 8 limited CEX execution — wiring `PILOT_ARMED` + `ELIGIBLE` into the registry/execution path with per-tenant/global breaker enforcement and disarm→flatten→revocation ordering.
