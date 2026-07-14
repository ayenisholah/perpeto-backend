# Changelog

All notable repository changes are recorded here using Keep a Changelog categories.

## [Unreleased]

### Added

- Initial backend repository governance, Rust workspace scaffold, API contract seed, container build, CI, public GHCR publishing, documentation synchronization, and paper-only staging deployment automation.

### Security

- Staging deployment validates paper mode and uses a pinned SSH host key; runtime trading secrets remain outside GitHub Actions.
