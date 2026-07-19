# Requirements Traceability

| Requirement | Planned component | Verification | Status |
|---|---|---|---|
| Spec 3.2 numeric rules | `funding-arb-domain` decimal newtypes | Unit/property tests for scale and rounding | Not started |
| Spec 5 execution guarantees | execution state machine and reconciler | Restart, retry, partial-fill and unknown-outcome tests | Not started |
| Spec 6 risk management | risk reservations and circuit breakers | Precedence and kill-switch property tests | Not started |
| Spec 8 architecture | workspace, services and connector boundary | Compile checks and architecture tests | Scaffolded |
| Spec 10 API contracts | OpenAPI and event schemas | Schema lint and compatibility diff | Scaffolded |
| Spec 10.2 / DEC-0007 / DEC-0008 social auth | provider-verification adapters, auth domain, PostgreSQL repositories and `axum` API | Provider/replay/bootstrap/approval/linking/deletion integration tests | Runtime adapters (`funding-arb-auth-service`), HTTP endpoints, and Postgres-backed CI auth-flow tests implemented; identity relinking and real-device E2E pending |
| DEC-0009 interim open registration | `PERPETO_OPEN_REGISTRATION` flag, auto-approve in `commit_exchange`, MFA waiver in `compute_next` | Domain + Postgres-backed open-registration exchange test (full-access, no MFA) and flag-off pending test | Implemented and covered by CI; reverts with v2 per-user isolation |
| Spec 11.3 / DEC-0007 mobile auth | native providers, auth state machine, Secure Store and route guards | State-machine, storage-failure, cancellation, accessibility and iPhone E2E | In progress |
| Spec 14 security | auth, secret store and signer policy | Security tests, scanning and external review | Auth invariant tests plus Ed25519 access tokens, hashed rotating refresh tokens, JWKS provider verification, and AES-GCM TOTP-secret encryption implemented; external review pending |
| Spec 15 deployment | Compose and staging deployment | Compose validation and paper-mode smoke test | Scaffolded |
| Spec 17 acceptance | CI and certification evidence | Required checks and signed evidence | In progress |
| Spec 18 CI/CD | GitHub Actions and GHCR | Workflow runs, SBOM, provenance and staging smoke | Scaffolded |
