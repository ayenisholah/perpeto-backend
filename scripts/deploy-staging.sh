#!/usr/bin/env bash
set -euo pipefail

: "${PERPETO_IMAGE:?PERPETO_IMAGE is required}"
test -f runtime.env || { echo "runtime.env is missing" >&2; exit 1; }
grep -Eq '^PERPETO_MODE=PAPER$' runtime.env || { echo "staging requires PERPETO_MODE=PAPER" >&2; exit 1; }

compose=(docker compose --env-file runtime.env -f deploy/compose/docker-compose.staging.yml)
# Optional host-specific layer, e.g. the AWS override with Caddy and CloudWatch
# logging (deploy/ec2/refresh.sh).
if [[ -n "${PERPETO_COMPOSE_OVERRIDE:-}" ]]; then
  compose+=(-f "$PERPETO_COMPOSE_OVERRIDE")
fi
printf '%s\n' "$PERPETO_IMAGE" > .pending-image
"${compose[@]}" config --quiet
"${compose[@]}" pull

if "${compose[@]}" ps --status running postgres --quiet | grep -q .; then
  mkdir -p backups
  "${compose[@]}" exec -T postgres sh -c 'pg_dump -U "$POSTGRES_USER" "$POSTGRES_DB"' | gzip > "backups/predeploy-$(date -u +%Y%m%dT%H%M%SZ).sql.gz"
fi

"${compose[@]}" up -d postgres redis --wait --wait-timeout 60

# Provision the migrator and runtime roles and set their passwords from
# runtime.env. Idempotent, so it runs on every deploy of a host that opts in;
# the staging Postgres has no init hook, unlike the dev stack.
if [[ "${PERPETO_PROVISION_DB_ROLES:-0}" == "1" ]]; then
  url_password() { grep -E "^$1=" runtime.env | tail -1 | sed -E 's#^[^=]+=postgres://[^:]+:([^@]+)@.*#\1#'; }
  migrator_password="$(url_password MIGRATE_DATABASE_URL)"
  app_password="$(url_password DATABASE_URL)"
  [[ "$migrator_password" =~ ^[A-Za-z0-9]+$ && "$app_password" =~ ^[A-Za-z0-9]+$ ]] || {
    echo "role passwords must be alphanumeric" >&2
    exit 1
  }
  {
    cat scripts/bootstrap-db-roles.sql
    printf "ALTER ROLE perpeto_migrator PASSWORD '%s';\n" "$migrator_password"
    printf "ALTER ROLE perpeto_app PASSWORD '%s';\n" "$app_password"
  } | "${compose[@]}" exec -T postgres sh -c 'psql -q -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB"'
fi
# Must be the `migrate` service, not `api` with a swapped entrypoint: the api
# service carries the runtime `DATABASE_URL` (`perpeto_app`), which owns nothing
# and cannot create tables, so migrating through it fails with "permission denied
# for schema public". Only `migrate` overrides the URL to the schema owner.
"${compose[@]}" run --rm migrate
"${compose[@]}" up -d --remove-orphans --wait --wait-timeout 60

for _ in $(seq 1 30); do
  if curl --fail --silent http://127.0.0.1:8081/readyz >/dev/null; then
    mv .pending-image .current-image
    echo "staging deployment is ready: $PERPETO_IMAGE"
    exit 0
  fi
  sleep 2
done

echo "staging readiness failed" >&2
"${compose[@]}" logs --tail=200 api engine reconciler preflight rewrap worker >&2 || true
exit 1
