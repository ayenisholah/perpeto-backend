# Staging Deployment

GitHub deploys only to a pre-existing Docker-ready staging VPS in paper mode. It does not create users, install packages, modify the firewall, configure DNS, provision TLS, or create per-client infrastructure.

## GitHub environment

Create a `staging` environment with `STAGING_SSH_PRIVATE_KEY` and the variables `STAGING_HOST`, `STAGING_PORT`, `STAGING_USER`, `STAGING_KNOWN_HOSTS`, `STAGING_DEPLOY_PATH`, and `STAGING_BASE_URL`. The SSH account must be non-root and restricted to the deployment directory and required Docker commands.

## VPS prerequisites

The deployment directory must already contain `runtime.env` with `SIGNEX_MODE=PAPER`, database and Redis values, and any test-only runtime secret references. Runtime trading secrets never enter GitHub. Public GHCR images require no registry login.

Deployments are serialized, digest-addressed, migration-first, health-checked, and fail closed. Rollback restores the previous compatible image and never arms entries. Production deployment is intentionally absent.

The staging API container listens on port `8080` internally and is bound only to `127.0.0.1:8081` on the VPS so it cannot conflict with existing services or bypass the host TLS reverse proxy. PostgreSQL uses the specification-pinned PostgreSQL 18.4 multi-platform image digest and its major-version-aware data layout. Deployment waits for PostgreSQL, Redis, and API health before reporting success.
