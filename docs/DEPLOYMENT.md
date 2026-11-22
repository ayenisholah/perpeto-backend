# Staging Deployment

This file documents the implemented paper staging deployment, then the M3 public-demo target (not yet implemented). There is no production deployment: LIVE is permanently fail-closed (DEC-0015).

GitHub deploys only to a pre-existing Docker-ready staging VPS in paper mode. It does not create users, install packages, modify the firewall, configure DNS, or provision TLS. The single operator-managed VPS runs one control plane. The application is tenant-aware (forced row-level security), and the deployment serves one operator tenant plus, from M3, one read-only demo tenant.

## GitHub environment

Create a `staging` environment with `STAGING_SSH_PRIVATE_KEY` and the variables `STAGING_HOST`, `STAGING_PORT`, `STAGING_USER`, `STAGING_KNOWN_HOSTS`, `STAGING_DEPLOY_PATH`, and `STAGING_BASE_URL`. The SSH account must be non-root and restricted to the deployment directory and required Docker commands.

## VPS prerequisites

The deployment directory must already contain `runtime.env` with `PERPETO_MODE=PAPER`, database and Redis values, and any test-only runtime secret references. Runtime trading secrets never enter GitHub. Public GHCR images require no registry login.

Until M2 lands, the paper engine replays fixtures. It defaults `PERPETO_SIM_START` to `2026-07-19T00:00:00Z` and advances simulated market time by `PERPETO_SIM_TICK_SECS=14400` on every loop, independently of the real `PERPETO_SCAN_INTERVAL_SECS` sleep. Override the start with a valid RFC 3339 timestamp or the tick with a positive integer; invalid values fail engine startup. M2 adds live public-data paper mode on wall-clock time (DEC-0016). Fixture replay then becomes an explicit selection, used for deterministic tests.

The M1 authentication runtime additionally requires the following in `runtime.env` (see `runtime.env.example`). Generate the signing key and deployment token with `scripts/dev-auth-env.sh`:

- `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB` — consumed by the Postgres container. This role is the superuser and owns every table, so no service may run as it.
- `MIGRATE_DATABASE_URL` — the `perpeto_migrator` schema owner, used only by `funding-arb-migrate`.
- `DATABASE_URL` — the `perpeto_app` runtime role for every service, e.g. `postgres://perpeto_app:...@postgres:5432/perpeto` (the `postgres` host is the compose service).

- `PERPETO_ACCESS_TOKEN_KEY` — base64 of a 32-byte Ed25519 seed used to sign access tokens.
- `PERPETO_TOKEN_ISSUER` — the public API origin (e.g. `https://api.perpeto.xyz`).
- `PERPETO_APPLE_BUNDLE_ID` — `com.signex.mobile` (audience for Apple identity tokens).
- `PERPETO_GOOGLE_IOS_CLIENT_ID`, `PERPETO_GOOGLE_WEB_CLIENT_ID` — accepted audiences for Google identity tokens (either may appear as `aud`).
- `PERPETO_BOOTSTRAP_TOKEN_HASH`, `PERPETO_BOOTSTRAP_TOKEN_EXPIRES_AT` — SHA-256 hex of the one-use deployment token and its RFC 3339 expiry; the first social sign-in exchanges the plaintext token to become Owner.
- `PERPETO_MFA_KEY` — base64 of a 32-byte AES-256-GCM key that encrypts stored TOTP secrets.
- `PERPETO_OPEN_REGISTRATION` — optional (default `false`). Set `true` only on the legacy shared v1 dev/TestFlight backend so each new social sign-in is admitted immediately to full access without pending approval or MFA (DEC-0009). Leave unset/`false` on any publicly reachable deployment. The public demo uses the read-only viewer (DEC-0017) instead.

`PERPETO_OPEN_REGISTRATION` and its universal privilege waiver are transitional, development-only behaviour.

## Database roles

Run `scripts/bootstrap-db-roles.sql` as the superuser once against a new database, before the first migration:

```bash
psql "postgres://perpeto:...@postgres:5432/perpeto" -v ON_ERROR_STOP=1 -f scripts/bootstrap-db-roles.sql
```

It creates the `perpeto_migrator` schema owner and the `perpeto_app` runtime role, and sets default privileges so tables added by later migrations stay reachable by `perpeto_app`. Both are `NOSUPERUSER NOBYPASSRLS`. This separation is mandatory, not stylistic: `assert_safe_runtime_role` refuses to start `api`, `engine`, `worker` or `rewrap` under a role that can bypass row-level security or owns a tenant table, because either condition silently disables tenant isolation.

## Local development

The development stack applies the same role script automatically through the Postgres init hook, then the API and workers run natively:

```bash
docker compose -f deploy/compose/docker-compose.dev.yml up -d
bash scripts/dev-run-api.sh
```

PostgreSQL 18 and Redis publish to `127.0.0.1` only; override the host ports with `PERPETO_DEV_DB_PORT` and `PERPETO_DEV_REDIS_PORT` if they collide. `dev-run-api.sh` generates `deploy/dev.env` (gitignored) on first run, migrates as the schema owner, and binds the API to `0.0.0.0:8080` so a phone on the same network can reach it. It prints a one-use deployment token that seats the first Owner account.

Building the API natively requires `pkg-config` and `libssl-dev`, because `webauthn-rs` links native OpenSSL. GitHub's runners already provide both, so CI needs no extra step.

Two things account for most local failures:

- **`address already in use` on 5432 usually means a second Docker daemon.** `docker context ls` shows which daemon plain `docker` commands target; containers on one daemon are invisible to the other while still holding the host port. Compose acts on the *active* context, so a stack started with an explicit `--context` will not be found without it.
- **The role bootstrap runs only on a fresh volume.** If `perpeto_migrator` or `perpeto_app` is missing, migrations fail with `permission denied for schema public`. Confirm with `docker compose -f deploy/compose/docker-compose.dev.yml exec postgres psql -U perpeto -d perpeto -c '\du'` and recreate with `down -v` rather than `up -d`.

## M3 public demo target

Not yet implemented. The demo is the staging deployment, made reviewable:

- **Staging tracks `main`.** The last successful staging deploy was `07cb3cc` (2026-07-30, fixture PAPER). On 2026-11-22 `https://api.perpeto.xyz/livez` returned 502 from the host proxy, so the first M3 task is to restore the stack and redeploy current `main`.
- **Live-data PAPER** (DEC-0016) with no venue credential anywhere in `runtime.env`.
- **A read-only demo viewer** into one seeded demo tenant (DEC-0017). `PERPETO_OPEN_REGISTRATION` and `PERPETO_DEV_AUTH` stay unset.
- **A hosted web build of the Expo client** pointed at `https://api.perpeto.xyz`, plus a TestFlight build from the existing iOS workflow.
- **One-command local start:** a Compose profile that brings up PostgreSQL, Redis, migrations, API and engine in live-data PAPER, documented as the README's 5-minute tour.

## Optional Vault backend

The Vault Transit store is implemented hardening and is not required for any milestone (DEC-0015). If it is deployed, its intended shape is:
- one TLS-protected node with integrated storage, private networks only;
- a 2-of-3 Shamir initialization;
- encrypt-only policy for the API and purpose-limited decrypt for the engine and reconciler.

Unseal shares and root tokens never appear in `runtime.env`, Compose, GitHub, images or backups. Without Vault, the transitional AES store holds the operator's testnet keys only (M4). Vault HA, auto-unseal and stronger host isolation are deferred (DEC-0018).

The `funding-arb-migrate` container runs `migrations/` before the API starts; the compose file gates the API on its successful completion.

Deployments are serialized, digest-addressed, migration-first, health-checked, and fail closed. Rollback restores the previous compatible image and never arms entries. Production deployment is intentionally absent and not planned.

The staging API container listens on port `8080` internally and is bound only to `127.0.0.1:8081` on the VPS so it cannot conflict with existing services or bypass the host TLS reverse proxy. PostgreSQL uses the specification-pinned PostgreSQL 18.4 multi-platform image digest and its major-version-aware data layout. Deployment waits for PostgreSQL, Redis, and API health before reporting success.
