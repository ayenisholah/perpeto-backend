#!/usr/bin/env bash
set -euo pipefail

: "${PERPETO_IMAGE:?PERPETO_IMAGE is required}"
test -f runtime.env || { echo "runtime.env is missing" >&2; exit 1; }
grep -Eq '^PERPETO_MODE=PAPER$' runtime.env || { echo "staging requires PERPETO_MODE=PAPER" >&2; exit 1; }

compose=(docker compose --env-file runtime.env -f deploy/compose/docker-compose.staging.yml)
printf '%s\n' "$PERPETO_IMAGE" > .pending-image
"${compose[@]}" config --quiet
"${compose[@]}" pull

if "${compose[@]}" ps --status running postgres --quiet | grep -q .; then
  mkdir -p backups
  "${compose[@]}" exec -T postgres sh -c 'pg_dump -U "$POSTGRES_USER" "$POSTGRES_DB"' | gzip > "backups/predeploy-$(date -u +%Y%m%dT%H%M%SZ).sql.gz"
fi

"${compose[@]}" up -d postgres redis --wait --wait-timeout 60
"${compose[@]}" run --rm --entrypoint funding-arb-migrate api
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
"${compose[@]}" logs --tail=200 api engine reconciler preflight rewrap worker >&2
exit 1
