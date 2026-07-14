# Changelog

All notable repository changes are recorded here using Keep a Changelog categories.

## [Unreleased]

### Added

- Initial backend repository governance, Rust workspace scaffold, API contract seed, container build, CI, public GHCR publishing, documentation synchronization, and paper-only staging deployment automation.

### Security

- Staging deployment validates paper mode and uses a pinned SSH host key; runtime trading secrets remain outside GitHub Actions.

### Fixed

- Applied canonical Rust formatting and completed the M0 OpenAPI metadata, response, and explicit public-health security declarations required by contract linting.

### Changed

- Moved project-control documentation under `docs/` and recorded the temporary solo-maintainer administrator bypass policy.
- Isolated the paper staging API on host loopback port 8081, aligned PostgreSQL staging with the specification-pinned 18.4 image digest and data layout, and made deployment wait for dependency health.
