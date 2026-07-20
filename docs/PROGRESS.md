# Backend Progress

## Snapshot

- Milestone: M3 — CEX connector certification
- Overall state: In progress (engineering implemented through evidence tooling)
- Last verified: 2026-07-20

## Completed

- Product specification reviewed and renamed for canonical use.
- Backend documentation hierarchy and decision log established.
- Executable Rust workspace and health-only service scaffolds created.
- Initial OpenAPI/event contracts and GitHub automation added.
- Public GitHub repository, required checks, branch protection, staging environment, and disabled-by-default automation switches configured.
- Backend CI, container build, SBOM/provenance publication, and public GHCR delivery verified green.
- M0 API-client packaging is deterministic and locally verifies compiled code plus bundled schemas.
- Accepted DEC-0007 and replaced the password flow in the canonical specification with Google/Apple identity and Perpeto-authoritative approval, MFA, sessions, and revocation.
- Added the M1A PostgreSQL schema, authentication invariant model/tests, OpenAPI 0.2.0 contract, and typed TypeScript client.
- Added M2 Slice 5a supervision: current-mark delta, two-tick corrective re-hedging, forecast/health exit decisions, manual/automatic exit reasons, engine persistence on an accelerated scenario clock, OpenAPI/client 0.7.0, and mobile re-hedge/exit monitoring.
- Added M2 Slice 5b: deterministic partial/unknown fill reconciliation and bounded recovery, `RECOVERING` lifecycle support, durable paper circuit breakers/outbox, daily-loss/drawdown and degraded-venue enforcement, emergency-control APIs, OpenAPI/client 0.8.0, and mobile controls/badges.
- Completed M2 Slices 6–7: immutable strategy versions and deterministic auto-entry, position attribution, portfolio/PnL aggregation, synthetic health heartbeats, deduplicated acknowledged alerts, OpenAPI/client 0.10.0, and the complete mobile monitoring control plane.
- Implemented M3 Slices 1–19 through the connector observability read path: fail-closed runtime modes, exact connector contracts/books, encrypted account configuration, Bybit/Binance/OKX adapters, durable sandbox coordination and reconciliation, live scanner wiring, masked account/order controls, and exact accounting APIs/client 0.13.0. Credentialed venue certification evidence remains pending.
- Started M2 Slice 1: `funding-arb-domain` market/forecast/opportunity types with a transparent non-ML forecaster and PRODUCT_SPEC §4.5 ranking (unit-tested); a `funding-arb-scanner-service` crate driven by deterministic fixtures with Postgres persistence (`0003_market_scan.sql`); the engine as a paper scan loop; a read-only `GET /api/v1/opportunities` endpoint; and the additive OpenAPI 0.3.0 / `@ayenisholah/perpeto-api-client@0.3.0` contract with a mobile Scanner screen.
- Started M2 Slice 2: richer opportunities (break-even funding, required capital, entry basis, leverage, per-leg forecast) and a `funding-arb-domain::risk` `RiskLimits` model (§6.2 safe-starter defaults) applied as scanner eligibility gates with per-opportunity paper reservations; the full §4.6 filter/sort set, `GET /api/v1/opportunities/{route_id}` detail, and `GET /api/v1/risk/limits`; OpenAPI 0.4.0 / client `0.4.0`; and a mobile filter/sort control, opportunity-detail sheet, and risk-limits card.
- Started M2 Slice 4: `funding-arb-domain::position` gains deterministic funding capture (`settle_funding`, §3.3 signs, N settlements elapsed) and a `close_paper` path (`OPEN→CLOSING→CLOSED`) that flattens both legs with reduce-only/sell-owned fills and records realized PnL (funding + trading PnL − fees); each leg now carries its mark/funding rate/interval. `execution-service` adds `ExecutionService::close`, `PositionsRepo::update`, and a `PositionDto::to_domain` reload; the API adds RBAC-gated, state-idempotent `POST /api/v1/positions/{id}/close`. OpenAPI 0.6.0 / client `0.6.0` (`closePosition`; `Position.funding_captured/realized_pnl/closed_at`, `PositionLeg.mark_price/funding_rate/funding_interval_hours`); a mobile Close action and realized-PnL/funding display on the Positions screen.
- Started M2 Slice 3: a `funding-arb-domain::position` open-path state machine (§5.2 `QUALIFIED→RESERVED→OPENING→OPEN|OPEN_FAILED`) and a deterministic paper-fill simulator (`open_paper`) that sizes both legs, records order intents/fills, and gates residual delta against the new §6.2 unmatched-exposure cap on `RiskLimits`; a `funding-arb-execution-service` crate (`ExecutionService::open` reusing the deterministic scanner/fixtures, `PositionsRepo`, `0005_paper_positions.sql`); RBAC-gated `POST /api/v1/positions` (OWNER/TRADER, `Idempotency-Key`-deduped), `GET /api/v1/positions`, and `GET /api/v1/positions/{id}`; OpenAPI 0.5.0 / client `0.5.0`; and a mobile Positions screen plus an "Open paper position" action on the opportunity detail sheet.

## Not completed

- Primary public/private WebSocket ingestion is not implemented; current connector streams poll REST snapshots and therefore do not satisfy the production real-time-path requirement.
- Bybit/Binance testnet, OKX demo, and production read-only shadow evidence require least-privilege operator credentials and the time-based 72-hour/14-day certification windows.

- The social-auth runtime adapters are implemented and deployed (`funding-arb-auth-service` + `axum` API). The shared v1 backend currently runs the DEC-0009 full-access registration waiver. DEC-0012 supersedes that model for the target: M4 will create personal tenants, add passkeys and prohibit universal privilege for live use.
- The completed M2 scanner, execution, strategy, and monitoring flows use deterministic paper fixtures only; no venue connector or production-ready live trading API exists. Position payloads persist as `f64`; lossless decimal persistence remains deferred.
- Shared-VPS tenant isolation, forced PostgreSQL RLS, tenant-aware background work, Vault Transit, WebAuthn passkeys, consumer credential enrollment, per-account live preflight and production CEX writes are not implemented. Their accepted target is M4; the existing AES master-key/admin-CLI path is M3-only transitional tooling.
- No venue has any certification evidence.
- Live trading is unavailable by design.

## Blockers

- The local Windows Rust linker is unavailable; compile, clippy, database-backed tests, and linked execution depend on GitHub Actions.
- VPS environment values and runtime configuration must be supplied before staging deployment.
- A least-privilege cross-repository token is required before documentation and API-update automation can be enabled.
- Client `0.2.0` publication and the permanent GitHub Packages pin await merge plus the versioned release workflow; mobile uses a verified vendored tarball meanwhile.

## Next verified task

Run the M3 connector certification workflow with least-privilege Bybit/Binance testnet and OKX demo credentials; live trading remains unavailable until the evidence is accepted.

After M3 evidence closes, begin M4 Slice 1 with personal-tenant/authenticated-context persistence. Do not expose a consumer credential mutation endpoint before RLS, passkey verification and encrypt-only Vault access land together.
