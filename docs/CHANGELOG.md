# Changelog

All notable repository changes are recorded here using Keep a Changelog categories.

## [Unreleased]

### Added

- M2 Slice 2 — opportunity detail, full filters, and risk reservations. `funding-arb-domain` gains a `risk` module (`RiskLimits` §6.2 safe-starter defaults + `RiskAssessment`) used as scanner eligibility gates and per-opportunity paper reservations, and richer `Opportunity` fields (break-even funding, required capital, entry basis, leverage, funding direction, per-leg forecast). The API adds the full §4.6 filter/sort set on `GET /api/v1/opportunities`, plus `GET /api/v1/opportunities/{route_id}` and `GET /api/v1/risk/limits`; opportunities persist as a serialized payload (`0004_opportunity_detail_risk.sql`). Published as OpenAPI/`@ayenisholah/perpeto-api-client` **0.4.0**. Live position/order reservations and circuit breakers remain deferred to the execution slice.
- M2 Slice 1 — deterministic paper opportunity scanner. `funding-arb-domain` gains `market`/`forecast`/`opportunity` modules (venues, funding observations, a transparent non-ML funding forecaster, and PRODUCT_SPEC §4.5 opportunity ranking with eligibility gates) using `rust_decimal`. A new `funding-arb-scanner-service` crate loads embedded market fixtures, ranks two-leg routes, and persists snapshots (`0003_market_scan.sql`); the `engine` binary runs it on an interval. The API adds a read-only, RBAC-gated `GET /api/v1/opportunities` (with `route_type`/`venue`/`min_net_apr`/`min_probability` filters), published as OpenAPI/`@ayenisholah/perpeto-api-client` **0.3.0**. Paper-only and network-free: no venue connectors, credentials, or execution.
- SemVer release tooling: `scripts/release.sh` bumps the shared `[workspace.package]` version, rolls `docs/CHANGELOG.md`, and tags `vX.Y.Z`; `.github/workflows/RELEASE.yml` publishes the GitHub Release from the changelog and promotes the immutable `sha-<commit>` image to `:vX.Y.Z`/`:latest`; policy documented in `docs/VERSIONING.md`. The API client remains versioned independently of the workspace.
- Interim open-registration mode (DEC-0009) behind `PERPETO_OPEN_REGISTRATION` (default off): on the shared v1 backend, each new Apple/Google sign-in is admitted immediately as an active account with the full `OWNER`/`TRADER`/`APPROVER` role set and no TOTP MFA, replacing the pending-approval gate. The strict DEC-0007 flow, approval endpoints, and bootstrap-Owner path are unchanged when the flag is off; version 2 restores gating with per-user backend isolation.
- M1 authentication HTTP runtime (`funding-arb-auth-service` crate plus an `axum` binary) implementing social challenge issuance, Apple/Google identity-token exchange with JWKS verification and PKCE, PENDING isolation, deployment-token Owner bootstrap, refresh rotation with reuse detection, TOTP enrollment/verification and recovery codes, session and identity listing, Owner access approval, and account deletion, all keyed by `(provider, provider_subject)` and never by email (DEC-0007, DEC-0008).
- A working `funding-arb-migrate` binary that applies `migrations/` via the `sqlx` migrator, a `migrate` compose service gating the API on completed migrations, and a Postgres-backed CI job running the database auth-flow tests.
- Initial backend repository governance, Rust workspace scaffold, API contract seed, container build, CI, public GHCR publishing, documentation synchronization, and paper-only staging deployment automation.
- Accepted the Google/Apple-only authentication decision, M1A persistence migration and domain invariants for pending approval, privileged MFA, one-use bootstrap/challenges, explicit identity linking, refresh-family reuse detection, final-Owner protection, and deletion.
- Added the additive OpenAPI and TypeScript authentication contract as client version `0.2.0`.
- Added API consumer documentation covering versioning, bearer security, RFC 9457 problem details, lifecycle semantics, endpoint groups, and contract-versus-runtime status.

### Security

- Staging deployment validates paper mode and uses a pinned SSH host key; runtime trading secrets remain outside GitHub Actions.
- Provider identity is keyed by immutable subject rather than email; access is pending by default, refresh credentials are device-bound/rotating, and retained deletion evidence uses an audit pseudonym.

### Fixed

- Applied canonical Rust formatting and completed the M0 OpenAPI metadata, response, and explicit public-health security declarations required by contract linting.

### Changed

- Rebranded Signex to **Perpeto**: renamed the `SIGNEX_*` runtime environment variables to `PERPETO_*`, the public API origin to `https://api.perpeto.xyz`, the Postgres role/database to `perpeto`, and the published contract package to `@ayenisholah/perpeto-api-client`. The Apple audience `com.signex.mobile` and the `signex://` OAuth redirect scheme are unchanged. Deployments must update `runtime.env` (new variable names, `DATABASE_URL`) and rename the database in lockstep — see `docs/DEPLOYMENT.md`.
- Moved project-control documentation under `docs/` and recorded the temporary solo-maintainer administrator bypass policy.
- Hardened the versioned API client to reject incompatible health payloads and verify compiled code and schemas before publication.
- Isolated the paper staging API on host loopback port 8081, aligned PostgreSQL staging with the specification-pinned 18.4 image digest and data layout, and made deployment wait for dependency health.
