# Backend Progress

## Snapshot

- Milestone: M0 — Governance and scaffolding
- Overall state: In progress
- Last verified: 2026-07-14

## Completed

- Product specification reviewed and renamed for canonical use.
- Backend documentation hierarchy and decision log established.
- Executable Rust workspace and health-only service scaffolds created.
- Initial OpenAPI/event contracts and GitHub automation added.
- Public GitHub repository, required checks, branch protection, staging environment, and disabled-by-default automation switches configured.
- Backend CI, container build, SBOM/provenance publication, and public GHCR delivery verified green.

## Not completed

- No database schema, auth, venue connector, scanner, risk engine, execution engine, ledger, or production-ready API exists.
- No venue has any certification evidence.
- Live trading is unavailable by design.

## Blockers

- Local Rust and Docker toolchains are unavailable; compilation and image validation depend on GitHub Actions.
- VPS environment values and runtime configuration must be supplied before staging deployment.
- A least-privilege cross-repository token is required before documentation and API-update automation can be enabled.

## Next verified task

Make the M0 CI run green, configure repository protections and staging environment values, then begin domain invariants under M1.
