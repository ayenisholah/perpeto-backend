# Versioning

Architecture documents may describe a future contract only when marked with its target milestone and implementation status. A documentation update does not reserve an API version or enable a runtime capability.

The Perpeto backend follows [Semantic Versioning 2.0.0](https://semver.org/).

## Two independently versioned artifacts

| Artifact | Version source | Meaning of a bump |
| --- | --- | --- |
| **Rust workspace** (all crates and `bins/*`) | `[workspace.package].version` in `Cargo.toml` (single shared version; every member sets `version.workspace = true`) | Behaviour of the deployed services and database schema |
| **API client** (`@ayenisholah/perpeto-api-client`) | `version` in `packages/api-client/package.json` | The OpenAPI/event contract surface consumed by the mobile app |

These two version lines are **decoupled** on purpose: a backend patch that changes no contract does not force an API client release, and an additive contract change can ship a new client without a backend redeploy.

## What each level means (workspace)

- **MAJOR** — a breaking runtime or operational change: a removed/renamed endpoint or field, an incompatible database migration, or a change to an environment-variable contract (`PERPETO_*`).
- **MINOR** — backwards-compatible additions: new endpoints/fields (behind the existing contract), new optional configuration, additive migrations.
- **PATCH** — backwards-compatible fixes with no contract or migration impact.

Pre-1.0.0 (`0.y.z`), treat **MINOR** as the breaking-change lever and **PATCH** as everything else, per SemVer §4.

## Cutting a backend release

1. Land all changes on `main` and record them under `## [Unreleased]` in [`docs/CHANGELOG.md`](CHANGELOG.md) using Keep a Changelog categories.
2. Run the release script from a clean `main`:

   ```bash
   scripts/release.sh <patch|minor|major>   # or an explicit X.Y.Z
   ```

   It bumps `[workspace.package].version`, refreshes `Cargo.lock`, moves the `[Unreleased]` notes into a dated `## [X.Y.Z]` section (opening a fresh `[Unreleased]`), commits `release: vX.Y.Z`, and creates an annotated `vX.Y.Z` tag.
3. Push the branch and tag:

   ```bash
   git push origin main && git push origin vX.Y.Z
   ```

4. The `vX.Y.Z` tag triggers [`.github/workflows/RELEASE.yml`](../.github/workflows/RELEASE.yml), which verifies the tag matches the workspace version, publishes a GitHub Release from the changelog section, and promotes the immutable `sha-<commit>` container image to `:vX.Y.Z` and `:latest`.

## Releasing the API client

Tenant identity is session-derived and never introduced as a caller-selectable header. Additive passkey, credential-preflight and live-eligibility resources receive a minor client version. Any release that changes authorization meaning, tenant visibility, secret handling or idempotency scope requires an explicit compatibility/security review and may require a major version even if its JSON shape appears additive.

Never publish generated credential endpoints ahead of the backend's forced-RLS, passkey and Vault boundary. Mobile must pin the exact client version and fail closed when its live-control contract is incompatible.

The client is published by [`PUBLISH_API.yml`](../.github/workflows/PUBLISH_API.yml). On a GitHub Release it publishes the version committed in `packages/api-client/package.json`; a manual `workflow_dispatch` can override with an explicit version. Bump `packages/api-client/package.json` (and re-pack the vendored tarball for the mobile app) whenever the OpenAPI contract in `schemas/` changes — this is separate from the workspace version.
