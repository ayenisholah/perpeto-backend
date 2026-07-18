# Changelog

All notable repository changes are recorded here using Keep a Changelog categories.

## [Unreleased]

### Added

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

- Moved project-control documentation under `docs/` and recorded the temporary solo-maintainer administrator bypass policy.
- Hardened the versioned API client to reject incompatible health payloads and verify compiled code and schemas before publication.
- Isolated the paper staging API on host loopback port 8081, aligned PostgreSQL staging with the specification-pinned 18.4 image digest and data layout, and made deployment wait for dependency health.
