# Requirements Traceability

| Requirement | Planned component | Verification | Status |
|---|---|---|---|
| Spec 3.2 numeric rules | `funding-arb-domain` `rust_decimal` rate/APR math | Unit tests for forecast, costing and ranking | In progress (M2-S1) |
| Spec 4.4 funding forecast | `funding-arb-domain::forecast` transparent non-ML predictor | Forecast determinism and p10/p90 band unit tests | In progress (M2-S1) |
| Spec 4.5–4.6 opportunity ranking, scanner and filters | `funding-arb-domain::opportunity` + `funding-arb-scanner-service`; API filters/sort | Ranking, eligibility-gate, scan-determinism and filter/sort tests | In progress (M2-S2) |
| Spec 4.5 / 11.6 opportunity feed and detail API | `GET /api/v1/opportunities`, `/{route_id}`, `/risk/limits`; OpenAPI 0.4.0; `PerpetoClient` | Client contract tests plus Postgres scan→read/detail integration | In progress (M2-S2) |
| Spec 5 execution guarantees | `funding-arb-domain::position` state machine + deterministic paper open/close/funding/re-hedge/recovery simulator; `ExecutionService`; engine persistence; position APIs | State-transition, partial/unknown reconciliation, recovery flatten, corrective-order, auto-exit, determinism and Postgres round-trip tests | Implemented for M2 paper execution through Slice 5b; live connectors/reconciliation remain M3 |
| Spec 6 risk management | `RiskLimits`, durable circuit breakers/outbox, emergency controls, engine loss/drawdown/venue-health enforcement | Gate, recovery cap, breaker precedence, RBAC, persistence and degraded-fixture tests | Implemented for M2 paper risk through Slice 5b; CI evidence pending |
| Spec 4.7 / 7.2 strategies and allocation | `funding-arb-domain::strategy`, `strategy-service`, versioned repository, engine auto-entry, strategy APIs | Admission/allocation/concurrency/cooldown determinism plus contract/mobile checks | Implemented for M2 paper automation |
| Spec 8 / 9.2 / 9.3 monitoring | `MonitoringRepo`, worker heartbeats/outbox alerts, portfolio/health/alerts APIs | PnL aggregation, deduplication, acknowledgement and mobile contract checks | Implemented for M2 paper monitoring |
| Spec 8 architecture | workspace, services and connector boundary | Compile checks and architecture tests | Scaffolded |
| Spec 8.5–8.6 connector contract and normalized instruments | `funding-arb-connectors` capability trait and exact venue-neutral models | Environment safety, terminal-state, fixture-contract and adapter suites | In progress (M3-S2 contract implemented; venue adapters pending) |
| Spec 9.2 venue accounts/instruments/fees/health | `funding-arb-venue-service`, `0010_venue_accounts.sql`, exact numeric metadata and masked account payloads | Migration plus PostgreSQL create/read/upsert round trips | In progress (M3-S4 persistence and M3-S5 secret store implemented; account API pending) |
| Spec 14 / DEC-0011 connector credentials | context-bound AES-256-GCM `SecretStore`, `0011_encrypted_credentials.sql`, audited stdin-only venue admin CLI | redaction/context-isolation unit tests plus PostgreSQL import/rotate/revoke integration | In progress (M3-S5 local encrypted store and admin CLI implemented; external KMS deferred) |
| Spec 10 API contracts | OpenAPI and event schemas | Schema lint and compatibility diff | Scaffolded |
| Spec 10.2 / DEC-0007 / DEC-0008 social auth | provider-verification adapters, auth domain, PostgreSQL repositories and `axum` API | Provider/replay/bootstrap/approval/linking/deletion integration tests | Runtime adapters (`funding-arb-auth-service`), HTTP endpoints, and Postgres-backed CI auth-flow tests implemented; identity relinking and real-device E2E pending |
| DEC-0009 interim open registration | `PERPETO_OPEN_REGISTRATION` flag, auto-approve in `commit_exchange`, MFA waiver in `compute_next` | Domain + Postgres-backed open-registration exchange test (full-access, no MFA) and flag-off pending test | Implemented and covered by CI; reverts with v2 per-user isolation |
| Spec 11.3 / DEC-0007 mobile auth | native providers, auth state machine, Secure Store and route guards | State-machine, storage-failure, cancellation, accessibility and iPhone E2E | In progress |
| Spec 14 security | auth, secret store and signer policy | Security tests, scanning and external review | Auth invariant tests plus Ed25519 access tokens, hashed rotating refresh tokens, JWKS provider verification, and AES-GCM TOTP-secret encryption implemented; external review pending |
| Spec 15 deployment | Compose and staging deployment | Compose validation and paper-mode smoke test | Scaffolded |
| Spec 17 acceptance | CI and certification evidence | Required checks and signed evidence | In progress |
| Spec 18 CI/CD | GitHub Actions and GHCR | Workflow runs, SBOM, provenance and staging smoke | Scaffolded |
