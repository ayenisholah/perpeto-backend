# Backend Progress

## Snapshot

- Milestone: M3 — CEX connector certification
- Overall state: In progress (M2 feature implementation present but exit gates open; M3 code present through evidence tooling)
- Last verified: 2026-07-21

## Completed or implemented

- Established the Rust workspace, PostgreSQL migrations, OpenAPI/event contracts, CI, container, staging, package-release and cross-repository documentation automation.
- Implemented the M1A Google/Apple authentication domain, PostgreSQL repositories, runtime provider adapters, HTTP endpoints, rotating sessions, TOTP/recovery flows and mobile authentication surfaces. The last verified staging deployment contains this pre-M2 baseline.
- Implemented the M2 paper scanner, opportunity/risk model, coordinated open/close/re-hedge/recovery lifecycle, circuit breakers, strategies, portfolio/PnL, health, alerts and corresponding mobile monitoring surfaces.
- Added M3 Slices 1–20: fail-closed `PAPER`/`SANDBOX`/`SHADOW` modes, normalized exact-decimal connector models and books, encrypted transitional credential administration, durable execution/reconciliation/accounting, Bybit/Binance/OKX REST adapters, masked account/order APIs, runtime wiring, observability and certification evidence tooling.
- Advanced the local OpenAPI/client contract to `@ayenisholah/perpeto-api-client@0.13.0`; local typecheck, 21 contract tests and package verification pass. The mobile app consumes the matching vendored tarball.
- Verified that documentation synchronization and mobile API-update automation are configured; `0.7.0` is the latest API-client version confirmed published to GitHub Packages.

## Exit gates and unverified work

- GitHub Backend CI for remote `main` at `6e59bad` is red: connector compilation reported 24 ownership/equality errors, so migrations and Rust tests did not run and the container/staging workflows were skipped. Local `main` contains the targeted borrow/equality repair, but it is not remotely compiled or CI-verified.
- The latest successful staging deployment is `e82347b`, which predates the M2/M3 implementation. Current staging therefore must not be described as containing the connector runtime.
- M2 remains open under the project completion standard: scanner, strategy, execution and monitoring wire/persistence DTOs still use `f64` for monetary, rate, price, quantity and PnL values. This conflicts with the exact-decimal invariant and requires a versioned contract/persistence remediation.
- Primary public/private WebSocket ingestion and connector-specific sequence recovery are not implemented; connector streams currently poll REST snapshots.
- No venue has accepted certification evidence. Credentialed Bybit/Binance testnet, OKX demo, production read-only shadow, reconciliation and the required 72-hour/14-day windows remain outstanding.
- M1A real-device, accessibility, security and full provider/replay integration evidence remains incomplete.
- Personal tenants, forced PostgreSQL RLS, tenant-aware jobs/caches/events, Vault Transit, WebAuthn passkeys, consumer credential enrollment, per-account live preflight and production CEX writes are M4 work and are not implemented.
- Live trading remains unavailable by design.

## Current blockers

- This Windows environment has Rust 1.97.0 but no usable MSVC linker, Docker or WSL, so linked Rust compile, Clippy and test evidence must remain pending until a suitable environment or a future CI run is available.
- Credentialed certification requires least-privilege operator test accounts and elapsed soak/shadow windows.
- API client `0.13.0` is locally verified but unpublished; `0.7.0` is the latest confirmed registry release.

## Next verified tasks

1. Run the local connector repair through locked backend CI before treating M3 code as build-verified.
2. Replace `f64` wire/persistence paths with lossless decimal representations and update the versioned API/mobile contract before closing M2.
3. Implement primary connector WebSockets, then run the credentialed M3 sandbox/demo/shadow certification matrix without production writes.
4. Begin M4 tenant context only after the earlier exit gates close; do not expose consumer credential mutation endpoints before forced RLS, passkey verification and encrypt-only Vault access land together.
