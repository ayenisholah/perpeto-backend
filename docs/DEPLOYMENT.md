# Staging Deployment

GitHub deploys only to a pre-existing Docker-ready staging VPS in paper mode. It does not create users, install packages, modify the firewall, configure DNS, provision TLS, or create per-client infrastructure.

## GitHub environment

Create a `staging` environment with `STAGING_SSH_PRIVATE_KEY` and the variables `STAGING_HOST`, `STAGING_PORT`, `STAGING_USER`, `STAGING_KNOWN_HOSTS`, `STAGING_DEPLOY_PATH`, and `STAGING_BASE_URL`. The SSH account must be non-root and restricted to the deployment directory and required Docker commands.

## VPS prerequisites

The deployment directory must already contain `runtime.env` with `SIGNEX_MODE=PAPER`, database and Redis values, and any test-only runtime secret references. Runtime trading secrets never enter GitHub. Public GHCR images require no registry login.

The M1 authentication runtime additionally requires the following in `runtime.env` (see `runtime.env.example`). Generate the signing key and deployment token with `scripts/dev-auth-env.sh`:

- `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` — consumed by the Postgres container.
- `DATABASE_URL` — e.g. `postgres://signex:...@postgres:5432/signex` (the `postgres` host is the compose service).
- `SIGNEX_ACCESS_TOKEN_KEY` — base64 of a 32-byte Ed25519 seed used to sign access tokens.
- `SIGNEX_TOKEN_ISSUER` — the public API origin (e.g. `https://signex-api.sholaayeni.xyz`).
- `SIGNEX_APPLE_BUNDLE_ID` — `com.signex.mobile` (audience for Apple identity tokens).
- `SIGNEX_GOOGLE_IOS_CLIENT_ID`, `SIGNEX_GOOGLE_WEB_CLIENT_ID` — accepted audiences for Google identity tokens (either may appear as `aud`).
- `SIGNEX_BOOTSTRAP_TOKEN_HASH`, `SIGNEX_BOOTSTRAP_TOKEN_EXPIRES_AT` — SHA-256 hex of the one-use deployment token and its RFC 3339 expiry; the first social sign-in exchanges the plaintext token to become Owner.
- `SIGNEX_MFA_KEY` — base64 of a 32-byte AES-256-GCM key that encrypts stored TOTP secrets.
- `SIGNEX_OPEN_REGISTRATION` — optional (default `false`). Set `true` on the shared v1 dev/TestFlight backend so each new social sign-in is admitted immediately to full access without pending approval or MFA (DEC-0009). Leave unset/`false` for isolated or production deployments.

The `funding-arb-migrate` container runs `migrations/` before the API starts; the compose file gates the API on its successful completion.

Deployments are serialized, digest-addressed, migration-first, health-checked, and fail closed. Rollback restores the previous compatible image and never arms entries. Production deployment is intentionally absent.

The staging API container listens on port `8080` internally and is bound only to `127.0.0.1:8081` on the VPS so it cannot conflict with existing services or bypass the host TLS reverse proxy. PostgreSQL uses the specification-pinned PostgreSQL 18.4 multi-platform image digest and its major-version-aware data layout. Deployment waits for PostgreSQL, Redis, and API health before reporting success.
