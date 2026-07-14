# Requirements Traceability

| Requirement | Planned component | Verification | Status |
|---|---|---|---|
| Spec 3.2 numeric rules | `funding-arb-domain` decimal newtypes | Unit/property tests for scale and rounding | Not started |
| Spec 5 execution guarantees | execution state machine and reconciler | Restart, retry, partial-fill and unknown-outcome tests | Not started |
| Spec 6 risk management | risk reservations and circuit breakers | Precedence and kill-switch property tests | Not started |
| Spec 8 architecture | workspace, services and connector boundary | Compile checks and architecture tests | Scaffolded |
| Spec 10 API contracts | OpenAPI and event schemas | Schema lint and compatibility diff | Scaffolded |
| Spec 14 security | auth, secret store and signer policy | Security tests, scanning and external review | Not started |
| Spec 15 deployment | Compose and staging deployment | Compose validation and paper-mode smoke test | Scaffolded |
| Spec 17 acceptance | CI and certification evidence | Required checks and signed evidence | In progress |
| Spec 18 CI/CD | GitHub Actions and GHCR | Workflow runs, SBOM, provenance and staging smoke | Scaffolded |
