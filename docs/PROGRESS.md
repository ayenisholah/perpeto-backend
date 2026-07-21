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

## Exit gates and unverified work

- GitHub Backend CI for remote `main` at `6e59bad` is red: connector compilation reported 24 ownership/equality errors, so migrations and Rust tests did not run and the container/staging workflows were skipped. Local `main` contains the targeted borrow/equality repair, but it is not remotely compiled or CI-verified.
- The latest successful staging deployment is `e82347b`, which predates the M2/M3 implementation. Current staging therefore must not be described as containing the connector runtime.
- The M2 exact-decimal source/contract remediation is implemented locally: scanner, strategy, execution and monitoring DTOs use `Decimal`, OpenAPI/client `0.15.0` retains decimal strings, fixtures preserve exact numeric lexemes, and mobile performs numeric conversion only for display. The exit remains open until Rust and PostgreSQL round-trip tests run in a usable build environment.
- Primary public/private WebSocket ingestion and connector-specific sequence recovery are implemented locally for Bybit, Binance and OKX, including bounded reconnecting transport and disconnect-to-reconciliation signaling. Linked Rust compilation and live venue evidence remain pending.
- No venue has accepted certification evidence. Credentialed Bybit/Binance testnet, OKX demo, production read-only shadow, reconciliation and the required 72-hour/14-day windows remain outstanding.
- M1A real-device, accessibility, security and full provider/replay integration evidence remains incomplete.
- The first forced-RLS/private-runtime isolation slice is implemented locally. PostgreSQL execution under distinct migration/runtime roles, broader adversarial API/job evidence, tenant-scoped caches/WebSocket delivery/exports, Vault Transit, WebAuthn passkeys, credential enrollment, account preflight and production CEX writes remain M4 work.
- Live trading remains unavailable by design.

## Current blockers

- This Windows environment has Rust 1.97.0 but no usable MSVC linker, Docker or WSL, so linked Rust compile, Clippy and test evidence must remain pending until a suitable environment or a future CI run is available.
- Credentialed certification requires least-privilege operator test accounts and elapsed soak/shadow windows.
- API client `0.15.0` is locally verified but unpublished; `0.7.0` is the latest confirmed registry release.

## Next verified tasks

1. Restore a usable local Rust linker or use an approved verification environment; compile and execute the new WebSocket/parser and tenant-isolation tests before treating either slice as build-verified.
2. Run the implemented lossless-decimal DTO/persistence and precision-regression tests before closing M2.
3. Run the credentialed M3 sandbox/demo/shadow certification matrix and timed windows without production writes.
4. Verify migration `0016` with distinct migrator/runtime roles and adversarial cross-tenant database/API/job tests. Keep passkey/Vault/credential enrollment as a later atomic security slice.
