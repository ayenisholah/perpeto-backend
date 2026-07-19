# Requirements Traceability

| Requirement | Planned component | Verification | Status |
|---|---|---|---|
| Spec 3.2 numeric rules | `funding-arb-domain` `rust_decimal` rate/APR math | Unit tests for forecast, costing and ranking | In progress (M2-S1) |
| Spec 4.4 funding forecast | `funding-arb-domain::forecast` transparent non-ML predictor | Forecast determinism and p10/p90 band unit tests | In progress (M2-S1) |
| Spec 4.5–4.6 opportunity ranking, scanner and filters | `funding-arb-domain::opportunity` + `funding-arb-scanner-service`; API filters/sort | Ranking, eligibility-gate, scan-determinism and filter/sort tests | In progress (M2-S2) |
| Spec 4.5 / 11.6 opportunity feed and detail API | `GET /api/v1/opportunities`, `/{route_id}`, `/risk/limits`; OpenAPI 0.4.0; `PerpetoClient` | Client contract tests plus Postgres scan→read/detail integration | In progress (M2-S2) |
| Spec 5 execution guarantees | `funding-arb-domain::position` open-path state machine + deterministic paper-fill simulator; `execution-service` (`open`, `PositionsRepo`); `POST`/`GET /api/v1/positions` | State-transition, paired-fill, residual-gate, determinism and Postgres round-trip/idempotency tests | In progress (M2-S3, paper open→OPEN; close, re-hedge, partial-fill/unknown-outcome recovery deferred to Slice 4) |
| Spec 6 risk management | `funding-arb-domain::risk` `RiskLimits` (safe-starter defaults) as scanner gates + paper reservations, plus the §6.2 unmatched-exposure cap enforced on paper open; `GET /api/v1/risk/limits` | Gate, reservation, residual-cap and default-template unit tests | In progress (M2-S3, limit model + gates + residual-delta cap; circuit breakers deferred to Slice 4) |
| Spec 8 architecture | workspace, services and connector boundary | Compile checks and architecture tests | Scaffolded |
| Spec 10 API contracts | OpenAPI and event schemas | Schema lint and compatibility diff | Scaffolded |
| Spec 10.2 / DEC-0007 / DEC-0008 social auth | provider-verification adapters, auth domain, PostgreSQL repositories and `axum` API | Provider/replay/bootstrap/approval/linking/deletion integration tests | Runtime adapters (`funding-arb-auth-service`), HTTP endpoints, and Postgres-backed CI auth-flow tests implemented; identity relinking and real-device E2E pending |
| DEC-0009 interim open registration | `PERPETO_OPEN_REGISTRATION` flag, auto-approve in `commit_exchange`, MFA waiver in `compute_next` | Domain + Postgres-backed open-registration exchange test (full-access, no MFA) and flag-off pending test | Implemented and covered by CI; reverts with v2 per-user isolation |
| Spec 11.3 / DEC-0007 mobile auth | native providers, auth state machine, Secure Store and route guards | State-machine, storage-failure, cancellation, accessibility and iPhone E2E | In progress |
| Spec 14 security | auth, secret store and signer policy | Security tests, scanning and external review | Auth invariant tests plus Ed25519 access tokens, hashed rotating refresh tokens, JWKS provider verification, and AES-GCM TOTP-secret encryption implemented; external review pending |
| Spec 15 deployment | Compose and staging deployment | Compose validation and paper-mode smoke test | Scaffolded |
| Spec 17 acceptance | CI and certification evidence | Required checks and signed evidence | In progress |
| Spec 18 CI/CD | GitHub Actions and GHCR | Workflow runs, SBOM, provenance and staging smoke | Scaffolded |
