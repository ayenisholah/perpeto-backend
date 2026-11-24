# Staging Deployment

This file documents the implemented paper staging deployment, then the M3 public-demo target (not yet implemented). There is no production deployment: LIVE is permanently fail-closed (DEC-0015).

PAPER staging runs on AWS in `eu-north-1` (DEC-0020), the only region the AWS organisation allows. One control plane runs on one EC2 host. The application is tenant-aware (forced row-level security), and the deployment serves one operator tenant plus, from M3, one read-only demo tenant.

## AWS staging host

The CDK app in `infra/` owns everything (`Perpeto-Staging` and `Perpeto-Deployer`):

- **Host:** one `t3.small` (x86_64, because CI publishes an amd64 image; CPU credits pinned to `standard`) on a pinned Amazon Linux 2023 AMI, with an Elastic IP, an encrypted 30 GiB gp3 root volume and IMDSv2. It runs the compose stack in `deploy/compose/docker-compose.staging.yml`, layered with `docker-compose.aws.yml`, which adds Caddy for TLS and sends every container's logs to the CloudWatch group `/perpeto/staging`.
- **Network:** a dedicated public-only VPC with no NAT gateway. Only 80 and 443 are open. There is no SSH: the operator uses SSM Session Manager, and deploys use SSM Run Command.
- **Secrets:** CloudFormation generates the three database passwords into Secrets Manager under `perpeto/staging/`. The first deploy generates the access-token key, the TOTP key, the credential master key and a one-use deployment token into `perpeto/staging/KEYS`. Nothing secret is in the repository or in GitHub except the deployer's access key.
- **Resilience:** an alarm recovers the instance onto healthy hardware when the system status check fails, and AWS Backup snapshots it daily (kept 7 days). Every deploy also takes a `pg_dump` into `/opt/perpeto/backups` first. The stack has termination protection and the instance has API termination disabled.
- **TLS:** Caddy serves `api.perpeto.xyz` and `<elastic-ip>.sslip.io`. The sslip.io name works as soon as the host is up; `api.perpeto.xyz` gets its certificate once its DNS `A` record points at the Elastic IP. DNS is at the registrar and changed by hand.

### How a deploy works

`.github/workflows/DEPLOY_STAGING.yml` runs after the container image for a `main` commit is published:

1. `cdk deploy --all` applies any infrastructure change.
2. `scripts/aws/deploy-host.sh` sends an SSM command that downloads `deploy/ec2/refresh.sh` from the repository at exactly that commit and runs it.
3. `refresh.sh` fetches the compose files, the Caddyfile, `deploy/ec2/staging.env` and the deploy scripts at that commit, renders `/opt/perpeto/runtime.env` from `staging.env` plus Secrets Manager, and runs `scripts/deploy-staging.sh`.
4. `deploy-staging.sh` backs up the database, provisions the `perpeto_migrator` and `perpeto_app` roles and their passwords (idempotent), migrates as the schema owner, starts the stack and waits for `/readyz`.
5. The workflow checks `https://<elastic-ip>.sslip.io/readyz` from outside.

Non-secret settings live in `deploy/ec2/staging.env` and change through a pull request.

PAPER staging runs `api`, `engine`, `worker`, `postgres`, `redis` and Caddy. The `reconciler`, `preflight` and `rewrap` workers refuse to start outside SANDBOX or SHADOW, so they sit behind the `connectors` compose profile and start only with `COMPOSE_PROFILES=connectors`. Without that profile, `deploy-staging.sh` removes any container left from an earlier run.

### Web platform (DEC-0021)

`app.perpeto.xyz` (and `app.<elastic-ip>.sslip.io` before DNS) serves the Next.js web platform from `perpeto-web`. Caddy sends every request to the `web` container on port 3000 with HSTS, `nosniff` and framing headers. The web server calls the API at `http://api:8080` over the compose network; the browser never talks to the API.

- **Configuration:** `refresh.sh` renders `/opt/perpeto/web.env` with only the API URL, the public origin, the session-cookie key (`PERPETO_WEB_SESSION_KEY`, generated once into `perpeto/staging/KEYS`) and the public Google and Apple web client identifiers. The read-only demo is on through `PERPETO_DEMO_ENABLED=true`.
- **Backend deploys:** `refresh.sh` keeps the web image last rolled out (`.current-web-image`), else `perpeto-web:main`, and starts the `web` profile only if that image can be pulled.
- **Web deploys:** `perpeto-web`'s deploy workflow uses the `perpeto-web-deployer` IAM user. That user may only send the `Perpeto-UpdateWeb` SSM document to the staging host. The document accepts `ghcr.io/ayenisholah/perpeto-web:sha-<full sha>` and runs `deploy/ec2/update-web.sh`, which recreates the `web` container alone. Create its key with `scripts/aws/create-deploy-key.sh perpeto-web-deployer ayenisholah/perpeto-web staging`.

### GitHub environment

The `staging` environment holds `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` for the `perpeto-github-deployer` IAM user, the variable `AWS_REGION=eu-north-1`, and the repository variable `STAGING_ENABLED=true`. The organisation's service control policy forbids GitHub OIDC, so the deployer is an IAM user. It can only assume the CDK bootstrap roles, read the `Perpeto-*` stack outputs, and run commands on the staging host. Create or rotate its key with `scripts/aws/create-deploy-key.sh`; the key goes from AWS straight into GitHub and is never printed.

### Operator tasks

- **Shell on the host:** `aws ssm start-session --target <InstanceId> --region eu-north-1`. The stack lives in `/opt/perpeto`.
- **Logs:** CloudWatch Logs group `/perpeto/staging`, one stream per container.
- **First Owner:** read `DEPLOYMENT_TOKEN` from the `perpeto/staging/KEYS` secret and enter it once in the app. It expires 30 days after the first deploy.
- **Manual deploy:** run the workflow with `image_sha`, or `scripts/aws/deploy-host.sh <sha> ghcr.io/ayenisholah/perpeto-backend:sha-<sha>` with credentials that can run SSM commands.
- **Infrastructure change:** `cd infra && npm ci && npx cdk diff`, then merge. CI type-checks and synthesizes the app without credentials, and validates the merged compose files.

Public GHCR images need no registry login. Runtime trading secrets never enter GitHub.

### PAPER market data

- **`PERPETO_PAPER_SOURCE`:** selects where PAPER reads market data (DEC-0016).
  - `live` (the default) takes a snapshot of keyless public Binance, Bybit and OKX data every scan, on wall-clock time. Each venue host must be reachable from the staging host (from `eu-north-1`, all three answered on 2026-11-23); an unreachable venue is reported `UNAVAILABLE` and the engine scans the rest. Check reachability with `funding-arb-connector-evidence snapshot` before relying on a venue.
  - `fixtures` replays the deterministic fixtures, described below.
- **`PERPETO_PAPER_UNDERLYINGS`:** limits the live universe. The default is `BTC,ETH`.
- **`PERPETO_MAX_SNAPSHOT_AGE_SECS`:** the newest snapshot age the API accepts for a paper open. The default is 90. Older snapshots are refused with 409 `STALE_MARKET_DATA`, so a stalled engine cannot leave the API filling against old books.

In fixture mode, the engine defaults `PERPETO_SIM_START` to `2026-07-19T00:00:00Z` and advances simulated market time by `PERPETO_SIM_TICK_SECS=14400` on every loop, independently of the real `PERPETO_SCAN_INTERVAL_SECS` sleep. Override the start with a valid RFC 3339 timestamp or the tick with a positive integer; invalid values fail engine startup. M2 adds live public-data paper mode on wall-clock time (DEC-0016). Fixture replay then becomes an explicit selection, used for deterministic tests.

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

The staging API container listens on port `8080` internally. Caddy reaches it on the compose network, and it is also bound to `127.0.0.1:8081` on the host for the deploy script's readiness check, so nothing reaches it except through TLS. PostgreSQL uses the specification-pinned PostgreSQL 18.4 multi-platform image digest and its major-version-aware data layout. Deployment waits for PostgreSQL, Redis, and API health before reporting success.
