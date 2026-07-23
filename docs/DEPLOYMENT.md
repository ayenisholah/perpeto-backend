# Staging Deployment

This file first documents the implemented paper/M3 staging deployment, then the accepted M4 production target. The target section is not yet implemented.

GitHub deploys only to a pre-existing Docker-ready staging VPS in paper mode. It does not create users, install packages, modify the firewall, configure DNS, or provision TLS. That single operator-managed VPS runs one shared control plane serving many personal tenants via PostgreSQL row-level security and per-tenant Vault encryption; deployment does not stand up separate infrastructure per trader.

## GitHub environment

Create a `staging` environment with `STAGING_SSH_PRIVATE_KEY` and the variables `STAGING_HOST`, `STAGING_PORT`, `STAGING_USER`, `STAGING_KNOWN_HOSTS`, `STAGING_DEPLOY_PATH`, and `STAGING_BASE_URL`. The SSH account must be non-root and restricted to the deployment directory and required Docker commands.

## VPS prerequisites

The deployment directory must already contain `runtime.env` with `PERPETO_MODE=PAPER`, database and Redis values, and any test-only runtime secret references. Runtime trading secrets never enter GitHub. Public GHCR images require no registry login.

The M2 paper engine defaults `PERPETO_SIM_START` to `2026-07-19T00:00:00Z` and advances simulated market time by `PERPETO_SIM_TICK_SECS=14400` on every loop, independently of the real `PERPETO_SCAN_INTERVAL_SECS` sleep. Override the start with a valid RFC 3339 timestamp or the tick with a positive integer; invalid values fail engine startup.

The M1 authentication runtime additionally requires the following in `runtime.env` (see `runtime.env.example`). Generate the signing key and deployment token with `scripts/dev-auth-env.sh`:

- `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` — consumed by the Postgres container.
- `DATABASE_URL` — e.g. `postgres://perpeto:...@postgres:5432/perpeto` (the `postgres` host is the compose service).
- `PERPETO_ACCESS_TOKEN_KEY` — base64 of a 32-byte Ed25519 seed used to sign access tokens.
- `PERPETO_TOKEN_ISSUER` — the public API origin (e.g. `https://api.perpeto.xyz`).
- `PERPETO_APPLE_BUNDLE_ID` — `com.signex.mobile` (audience for Apple identity tokens).
- `PERPETO_GOOGLE_IOS_CLIENT_ID`, `PERPETO_GOOGLE_WEB_CLIENT_ID` — accepted audiences for Google identity tokens (either may appear as `aud`).
- `PERPETO_BOOTSTRAP_TOKEN_HASH`, `PERPETO_BOOTSTRAP_TOKEN_EXPIRES_AT` — SHA-256 hex of the one-use deployment token and its RFC 3339 expiry; the first social sign-in exchanges the plaintext token to become Owner.
- `PERPETO_MFA_KEY` — base64 of a 32-byte AES-256-GCM key that encrypts stored TOTP secrets.
- `PERPETO_OPEN_REGISTRATION` — optional (default `false`). Set `true` only on the legacy shared v1 dev/TestFlight backend so each new social sign-in is admitted immediately to full access without pending approval or MFA (DEC-0009). Leave unset/`false` for production and all M4 environments.

`PERPETO_OPEN_REGISTRATION` and its universal privilege waiver are transitional only. DEC-0012 forbids that authorization model for the M4 pilot.

## M4 shared-VPS target

The production Compose topology adds one TLS-protected HashiCorp Vault node with integrated durable storage. It serves Transit only to private application networks. PostgreSQL remains shared but all tenant tables use forced row-level security and non-bypass application roles. The public API domain serves the app; PostgreSQL, Redis, Vault, observability and operator endpoints expose no public ports.

Vault bootstrap is an operator procedure: initialize once with a 2-of-3 Shamir quorum, distribute shares to separate custodians/recovery locations, revoke the initial root token after policies/auth are configured, and test sealed restart and recovery. Unseal shares, root/recovery tokens and trader credentials must never appear in `runtime.env`, Compose, GitHub, container images, database backups or app configuration.

API receives encrypt-only Transit policy. Engine and reconciler receive purpose-limited decrypt policies and short renewable leases. Exchange credentials are enrolled by traders through the passkey-authorized API; they are not environment variables or routine CLI input after M4.

The initial M4 pilot accepts one VPS and therefore one host compromise/availability domain. A restart leaves Vault sealed and new live risk blocked until two custodians unseal it. M6 owns Vault HA or managed auto-unseal, stronger workload isolation, tested DR and static exchange egress where IP allowlisting is supported; M7 owns `CERTIFIED_LIVE` rollout.

Production readiness must prove: tenant RLS is forced; missing tenant context fails closed; backups contain ciphertext but no shares; Vault policy tests pass; live mode cannot arm without account preflight and passkey proof; and the sealed-state alert/runbook works.

The `funding-arb-migrate` container runs `migrations/` before the API starts; the compose file gates the API on its successful completion.

Deployments are serialized, digest-addressed, migration-first, health-checked, and fail closed. Rollback restores the previous compatible image and never arms entries. Production deployment is intentionally absent.

The staging API container listens on port `8080` internally and is bound only to `127.0.0.1:8081` on the VPS so it cannot conflict with existing services or bypass the host TLS reverse proxy. PostgreSQL uses the specification-pinned PostgreSQL 18.4 multi-platform image digest and its major-version-aware data layout. Deployment waits for PostgreSQL, Redis, and API health before reporting success.
