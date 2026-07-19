# Backend Progress

## Snapshot

- Milestone: M2 Slice 2 — opportunity detail, full filters, and risk reservations (M1A auth + Slice 1 scanner complete)
- Overall state: In progress
- Last verified: 2026-07-19

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

## Not completed

- The social-auth runtime adapters are implemented and deployed (`funding-arb-auth-service` + `axum` API). The shared v1 backend runs with `PERPETO_OPEN_REGISTRATION` enabled (DEC-0009), so new sign-ins get immediate full access; per-user backend isolation and the restored gated-approval/MFA flow are the v2 objective.
- The scanner produces opportunities from deterministic fixtures only; no venue connector, risk engine, execution engine, ledger, or production-ready trading API exists.
- No venue has any certification evidence.
- Live trading is unavailable by design.

## Blockers

- Rust type-checking is available locally, but Windows test linking is blocked because the installed `link.exe` is not the MSVC linker; linked test execution depends on GitHub Actions.
- VPS environment values and runtime configuration must be supplied before staging deployment.
- A least-privilege cross-repository token is required before documentation and API-update automation can be enabled.
- Client `0.2.0` publication and the permanent GitHub Packages pin await merge plus the versioned release workflow; mobile uses a verified vendored tarball meanwhile.

## Next verified task

Complete M2 Slice 1: add the Postgres scan→read integration test, run the compose stack end-to-end (`migrate` + `engine` + `api`) against `GET /api/v1/opportunities`, and publish `@ayenisholah/perpeto-api-client@0.3.0`. Then begin Slice 2 (opportunity detail, filters, and risk reservations).
