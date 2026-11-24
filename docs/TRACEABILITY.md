# Requirements Traceability

Re-baselined to the DEC-0015 roadmap on 2026-11-22. **Milestone** is the milestone that owns the remaining work. **Hardening** marks implemented isolation and security work that is kept but is not a release gate. **Deferred** marks DEC-0018 scope.

| Requirement | Component | Verification | Milestone | Status |
|---|---|---|---|---|
| Spec 3.2 numeric rules | `funding-arb-domain` `rust_decimal` math; decimal-string wire and persistence DTOs | Unit tests plus lossless JSON and PostgreSQL round trips | M1 | Verified |
| Spec 4.4 funding forecast | `funding-arb-domain::forecast`, a transparent non-ML predictor | Determinism and p10/p90 band tests | M1 | Verified |
| Spec 4.5–4.6 ranking, scanner and filters | `funding-arb-domain::opportunity`, `funding-arb-scanner-service`, API filters and sort | Ranking, eligibility, scan-determinism and filter/sort tests | M1 | Verified |
| Spec 4.5 / 11.6 opportunity feed and detail | `GET /opportunities`, `/opportunities/{route_id}`, `/risk/limits` | Client contract tests and PostgreSQL scan → read integration | M1 | Verified (client `0.22.0`) |
| Spec 5 execution guarantees | `funding-arb-domain::position` state machine; deterministic paper open, close, funding, re-hedge and recovery; `ExecutionService` | State-transition, partial/unknown reconciliation, recovery-flatten and determinism tests | M1 | Verified in PAPER |
| Spec 6 risk management | `RiskLimits`, durable circuit breakers and outbox, emergency controls, loss/drawdown/venue-health enforcement | Gate, recovery-cap, breaker-precedence and cross-tenant breaker tests | M1 | Verified |
| Spec 4.7 / 7.1 strategies and allocation | `funding-arb-domain::strategy`, `strategy-service`, versioned repository, engine auto-entry | Admission, allocation, concurrency and cooldown tests | M1 | Verified in PAPER |
| Spec 7.2–7.3 compounding and rebalancing | — | — | Deferred | Not implemented (DEC-0018) |
| Spec 8 / 9.2–9.3 monitoring | `MonitoringRepo`, worker heartbeats, outbox alerts, portfolio, health and alerts APIs | PnL aggregation, deduplication and acknowledgement tests | M1 | Verified in PAPER |
| Spec 8 architecture | Workspace, services and connector boundary | fmt, Clippy (`-D warnings`), 136-test suite | M1 | Verified |
| Spec 8.5–8.6 connector contract and instruments | `funding-arb-connectors` capability trait and venue-neutral models | Environment safety, terminal-state and fixture-contract suites | M1 | Verified (fixtures) |
| Spec 8.5 order-book integrity | Snapshot-gated `BookState` with overlap, duplicate and sequence-gap policy | Gap-reset and executable-VWAP tests; fixture replay | M2 | Implemented, including reconnect re-seeding on every venue and fail-closed OKX sequence resets. Live public-stream evidence remains |
| DEC-0016 live-data paper mode | Engine `MarketLeg` from keyless public `market_stream` and `current_funding`; paper fills against live depth; wall-clock time | Public-payload parser tests, stale-feed blocking test, no-credential-path test, 24-hour staging run | M2 | **Next** |
| Spec 8.5 Bybit, Binance and OKX connectors | V5 / Spot+USDⓈ-M / V5 REST and public/private WebSockets with sequence recovery | Hashed fixtures, parser and reset tests; public probe (M2); testnet/demo execution (M4) | M2, M4 | Implemented. Public probe and testnet evidence pending |
| Spec 5.5–5.6 sandbox idempotency | Durable `ConnectorExecutionCoordinator`, persisted payload hashes, unique client IDs, query-before-retry | Duplicate-submission and unknown-outcome tests; testnet cycle | M4 | Implemented. Testnet evidence pending |
| Spec 8.7 connector reconciliation | Read-only `ReconciliationService`, accounting snapshots, discrepancy reports | Order comparison, duplicate-fill and pagination tests; PostgreSQL round trips | M4 | Implemented. Testnet reconciliation report pending |
| Spec 9.2 / 11.12 venue accounts and administration | `funding-arb-venue-service`, masked reads, configuration tests, disable | RBAC, schema lint, secret-field absence | M1 | Verified |
| Spec 10 API contracts | OpenAPI 3.1 and event schemas; generated TypeScript client | Schema lint and compatibility diff | M1 | Verified at `0.22.0` (vendored). Registry publication lags at `0.7.0` |
| Spec 10.2 / DEC-0007 / DEC-0008 social auth | Provider adapters, auth domain, repositories, `axum` API | Provider, replay, bootstrap, approval, linking and deletion tests | M1 | Verified. Real-device end-to-end testing pending (M3) |
| DEC-0017 read-only demo viewer | `users.demo` (migration `0020`), `POST /api/v1/demo/session`, the `require_auth` read-only guard, the seeded demo tenant and strategy (`bins/api/src/demo.rs`) | `demo_authorization_tests`: every protected mutation parsed from `routes.rs` returns 403 `DEMO_READ_ONLY`, the demo surfaces read 200, and the endpoint is absent when disabled | M3 | Implemented |
| DEC-0009 open registration | `PERPETO_OPEN_REGISTRATION` | Open-registration exchange tests | — | Development-only. Never enabled on the public demo |
| DEC-0014 development identity verifier | `DevIdentityVerifier`, PAPER-only and refuses to start otherwise | Mode-gate test | — | Development-only |
| DEC-0012 personal tenants and forced RLS | Session-derived tenant context, tenant-aware constraints, repositories and jobs, forced RLS | Adversarial cross-tenant tests under a non-`BYPASSRLS` role; fail as superuser | Hardening | Verified |
| DEC-0012 passkeys | WebAuthn registration and assertion; one-use action proofs | Passkey isolation suite | Hardening | Verified in the database. Native ceremony optional, flag-gated |
| DEC-0012 credential store | AES `SecretStore` (default); `VaultTransitSecretStore` (optional); rewrap worker | Redaction and context tests; AES rewrap test | Hardening | AES verified. Vault store compiles and is unverified against a live Vault (optional) |
| Credential lifecycle and preflight | Passkey-gated enroll, rotate, revoke and delete; preflight endpoint and worker | Enrollment, deletion, rewrap and preflight-worker suites | Hardening | Verified |
| Pilot gate and live-risk gate | `PilotState`, `permit_live_risk`, disarm → flatten | Domain pipeline and `pilot_state` tests | Hardening | Verified. Inert by design: LIVE is locked |
| Spec 14.7 audit integrity | Hash-chained `audit_events`, owner-only `GET /audit` | `audit_chain` tamper-detection test | Hardening | Verified |
| Spec 17.6 route certification | `connector_certification_evidence`, certification states | Evidence CLI | Deferred | Evidence tooling reused for M2 and M4 (`CONNECTOR_EVIDENCE.md`). Certification is not pursued |
| Spec 4.1 / 14.4 DEX venues and signer | `bins/signer` scaffold | — | Deferred | Not extended (DEC-0018) |
| Spec 11 mobile client | Expo app, five tabs, design system, generated client | Lint, typecheck, Vitest | M1, M3 | M1 verified. Web build and TestFlight in M3 |
| Spec 15 deployment | Compose and staging workflow | Compose validation and paper smoke test | M3 | Automation implemented. Last successful deploy `07cb3cc` (2026-07-30). The API returned 502 on 2026-11-22 |
| Spec 18 CI/CD | GitHub Actions and GHCR | Workflow runs and staging smoke | M1 | Verified: Backend CI, container and staging deploy green on `main` at `07cb3cc` |
