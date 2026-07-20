# Backend Progress

## Snapshot

- Milestone: M2 Slice 4 — paper position close and funding capture (M1A auth + Slice 1 scanner + Slice 2 detail/filters/risk + Slice 3 open lifecycle complete)
- Overall state: In progress
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
- Started M2 Slice 1: `funding-arb-domain` market/forecast/opportunity types with a transparent non-ML forecaster and PRODUCT_SPEC §4.5 ranking (unit-tested); a `funding-arb-scanner-service` crate driven by deterministic fixtures with Postgres persistence (`0003_market_scan.sql`); the engine as a paper scan loop; a read-only `GET /api/v1/opportunities` endpoint; and the additive OpenAPI 0.3.0 / `@ayenisholah/perpeto-api-client@0.3.0` contract with a mobile Scanner screen.
- Started M2 Slice 2: richer opportunities (break-even funding, required capital, entry basis, leverage, per-leg forecast) and a `funding-arb-domain::risk` `RiskLimits` model (§6.2 safe-starter defaults) applied as scanner eligibility gates with per-opportunity paper reservations; the full §4.6 filter/sort set, `GET /api/v1/opportunities/{route_id}` detail, and `GET /api/v1/risk/limits`; OpenAPI 0.4.0 / client `0.4.0`; and a mobile filter/sort control, opportunity-detail sheet, and risk-limits card.
- Started M2 Slice 4: `funding-arb-domain::position` gains deterministic funding capture (`settle_funding`, §3.3 signs, N settlements elapsed) and a `close_paper` path (`OPEN→CLOSING→CLOSED`) that flattens both legs with reduce-only/sell-owned fills and records realized PnL (funding + trading PnL − fees); each leg now carries its mark/funding rate/interval. `execution-service` adds `ExecutionService::close`, `PositionsRepo::update`, and a `PositionDto::to_domain` reload; the API adds RBAC-gated, state-idempotent `POST /api/v1/positions/{id}/close`. OpenAPI 0.6.0 / client `0.6.0` (`closePosition`; `Position.funding_captured/realized_pnl/closed_at`, `PositionLeg.mark_price/funding_rate/funding_interval_hours`); a mobile Close action and realized-PnL/funding display on the Positions screen.
- Started M2 Slice 3: a `funding-arb-domain::position` open-path state machine (§5.2 `QUALIFIED→RESERVED→OPENING→OPEN|OPEN_FAILED`) and a deterministic paper-fill simulator (`open_paper`) that sizes both legs, records order intents/fills, and gates residual delta against the new §6.2 unmatched-exposure cap on `RiskLimits`; a `funding-arb-execution-service` crate (`ExecutionService::open` reusing the deterministic scanner/fixtures, `PositionsRepo`, `0005_paper_positions.sql`); RBAC-gated `POST /api/v1/positions` (OWNER/TRADER, `Idempotency-Key`-deduped), `GET /api/v1/positions`, and `GET /api/v1/positions/{id}`; OpenAPI 0.5.0 / client `0.5.0`; and a mobile Positions screen plus an "Open paper position" action on the opportunity detail sheet.

## Not completed

- The social-auth runtime adapters are implemented and deployed (`funding-arb-auth-service` + `axum` API). The shared v1 backend runs with `PERPETO_OPEN_REGISTRATION` enabled (DEC-0009), so new sign-ins get immediate full access; per-user backend isolation and the restored gated-approval/MFA flow are the v2 objective.
- The scanner produces opportunities from deterministic fixtures only; no venue connector, double-entry ledger, or production-ready trading API exists. Paper positions open, capture funding, and close through a deterministic simulator (no venue connectivity); re-hedge, auto-exit triggers, and partial-fill/unknown-outcome recovery are deferred to a later slice. Position payloads persist as `f64` (Slice 3/4 fidelity); lossless decimal persistence is deferred to the ledger slice.
- No venue has any certification evidence.
- Live trading is unavailable by design.

## Blockers

- Rust type-checking is available locally, but Windows test linking is blocked because the installed `link.exe` is not the MSVC linker; linked test execution depends on GitHub Actions.
- VPS environment values and runtime configuration must be supplied before staging deployment.
- A least-privilege cross-repository token is required before documentation and API-update automation can be enabled.
- Client `0.2.0` publication and the permanent GitHub Packages pin await merge plus the versioned release workflow; mobile uses a verified vendored tarball meanwhile.

## Next verified task

Verify M2 Slice 4 in CI (workspace compile/clippy/tests, including the Postgres `positions` close/update round-trip), run the compose stack end-to-end (`migrate` + `engine` + `api`): `POST /positions` → `OPEN`, `POST /positions/{id}/close` → `200 CLOSED` with `funding_captured`/`realized_pnl`, repeat close → same `CLOSED`, non-trader → `403`. Publish `@ayenisholah/perpeto-api-client@0.6.0`. Then begin Slice 5 (re-hedge, auto-exit triggers, and partial-fill/unknown-outcome recovery — needs moving marks).
