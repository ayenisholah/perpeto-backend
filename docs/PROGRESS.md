# Backend Progress

## Snapshot

- Milestone: M1A — Google/Apple authentication and secure sessions
- Overall state: In progress
- Last verified: 2026-07-14

## Completed

- Product specification reviewed and renamed for canonical use.
- Backend documentation hierarchy and decision log established.
- Executable Rust workspace and health-only service scaffolds created.
- Initial OpenAPI/event contracts and GitHub automation added.
- Public GitHub repository, required checks, branch protection, staging environment, and disabled-by-default automation switches configured.
- Backend CI, container build, SBOM/provenance publication, and public GHCR delivery verified green.
- M0 API-client packaging is deterministic and locally verifies compiled code plus bundled schemas.
- Accepted DEC-0007 and replaced the password flow in the canonical specification with Google/Apple identity and Signex-authoritative approval, MFA, sessions, and revocation.
- Added the M1A PostgreSQL schema, authentication invariant model/tests, OpenAPI 0.2.0 contract, and typed TypeScript client.

## Not completed

- The social-auth runtime adapters are implemented and deployed (`funding-arb-auth-service` + `axum` API). The shared v1 backend runs with `PERPETO_OPEN_REGISTRATION` enabled (DEC-0009), so new sign-ins get immediate full access; per-user backend isolation and the restored gated-approval/MFA flow are the v2 objective.
- No venue connector, scanner, risk engine, execution engine, ledger, or production-ready trading API exists.
- No venue has any certification evidence.
- Live trading is unavailable by design.

## Blockers

- Rust type-checking is available locally, but Windows test linking is blocked because the installed `link.exe` is not the MSVC linker; linked test execution depends on GitHub Actions.
- VPS environment values and runtime configuration must be supplied before staging deployment.
- A least-privilege cross-repository token is required before documentation and API-update automation can be enabled.
- Client `0.2.0` publication and the permanent GitHub Packages pin await merge plus the versioned release workflow; mobile uses a verified vendored tarball meanwhile.

## Next verified task

Implement the provider-verification/cryptographic adapters and transactional PostgreSQL HTTP handlers, then merge, publish `0.2.0`, and replace the mobile vendored tarball with the exact GitHub Packages release.
