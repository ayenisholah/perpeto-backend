#!/usr/bin/env bash
# M4 SANDBOX runtime on the staging host (testnet and demo venues only). It runs
# as root through SSM Run Command, sent by scripts/aws/sandbox.sh. refresh.sh
# puts it on the host at each deploy.
#
#   sandbox.sh init                          database, roles, migrations, keys, tenant
#   sandbox.sh import [NAME ...]             BYBIT BINANCE_SPOT BINANCE_PERP OKX (default: all filled)
#   sandbox.sh evidence                      credentialed reads + reconciliation, no writes
#   sandbox.sh open    VENUE UNDERLYING NOTIONAL
#   sandbox.sh close   VENUE
#   sandbox.sh cycle   VENUE UNDERLYING NOTIONAL   detached; follow with `logs VENUE`
#   sandbox.sh recover VENUE UNDERLYING [FAULT_MS]
#   sandbox.sh reconcile
#   sandbox.sh report  VENUE SINCE_RFC3339   prints the Markdown report
#   sandbox.sh json    VENUE                 prints the JSON report
#   sandbox.sh status | logs VENUE
#
# SANDBOX is a separate stack from the PAPER demo:
# - its own database, `perpeto_sandbox`, in the same PostgreSQL, behind the
#   same two roles;
# - its own AES master key and operator tenant, kept in perpeto/sandbox/KEYS;
# - one-shot containers of the deployed backend image, with no public port.
# The PAPER services and the demo are never touched.
set -euo pipefail

dir=/opt/perpeto
cd "$dir"
# shellcheck disable=SC1091
. /etc/perpeto.env
[[ -f runtime.env && -f .current-image ]] || {
  echo "sandbox.sh: the backend stack has not been deployed on this host" >&2
  exit 1
}
image="$(cat .current-image)"
network=perpeto-staging_default
state="$dir/sandbox"
install -d -m 700 "$state"
install -d -m 700 -o 10001 -g 10001 "$state/evidence"
compose=(env PERPETO_IMAGE="$image" docker compose --env-file runtime.env
  -f deploy/compose/docker-compose.staging.yml -f deploy/compose/docker-compose.aws.yml)

secret() {
  aws secretsmanager get-secret-value --region "$AWS_REGION" \
    --secret-id "perpeto/sandbox/$1" --query SecretString --output text
}

# The staging URLs with the database swapped for perpeto_sandbox.
sandbox_url() {
  grep -E "^$1=" runtime.env | tail -n 1 | cut -d= -f2- | sed -E 's#/perpeto$#/perpeto_sandbox#'
}

psql_admin() {
  "${compose[@]}" exec -T postgres sh -c \
    "psql -q -v ON_ERROR_STOP=1 -U \"\$POSTGRES_USER\" -d ${1:-\$POSTGRES_DB}"
}

# Render the env file every sandbox container reads. It holds the sandbox key,
# so it is root-only and never printed.
render_env() {
  local keys
  keys="$(secret KEYS)"
  [[ -n "$(jq -r '.PERPETO_SECRET_MASTER_KEY // empty' <<<"$keys")" ]] || {
    echo "sandbox.sh: run init first" >&2
    exit 1
  }
  umask 077
  {
    echo "PERPETO_MODE=SANDBOX"
    echo "DATABASE_URL=$(sandbox_url DATABASE_URL)"
    echo "PERPETO_SECRET_MASTER_KEY=$(jq -r .PERPETO_SECRET_MASTER_KEY <<<"$keys")"
    jq -r '.PERPETO_TENANT_ID // empty | "PERPETO_TENANT_ID=" + .' <<<"$keys"
  } > "$state/sandbox.env.tmp"
  mv "$state/sandbox.env.tmp" "$state/sandbox.env"
}

# One container of the deployed image, on the stack's network, hardened like
# the compose services, logging to CloudWatch as sandbox-<label>.
run() {
  local label="$1" entrypoint="$2"
  shift 2
  docker run --rm -i --network "$network" --env-file "$state/sandbox.env" \
    --read-only --cap-drop ALL --security-opt no-new-privileges:true --init \
    --log-driver awslogs --log-opt awslogs-region="$AWS_REGION" \
    --log-opt awslogs-group="$PERPETO_LOG_GROUP" --log-opt tag="sandbox-$label" \
    "${RUN_EXTRA[@]}" --entrypoint "$entrypoint" "$image" "$@"
}
RUN_EXTRA=()

venue_arg() {
  case "${1:-}" in
    BYBIT | BINANCE | OKX) ;;
    *) echo "VENUE must be BYBIT, BINANCE or OKX" >&2; exit 2 ;;
  esac
}

init() {
  local exists
  exists="$("${compose[@]}" exec -T postgres sh -c \
    'psql -tA -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT 1 FROM pg_database WHERE datname = '"'"'perpeto_sandbox'"'"'"')"
  if [[ "$exists" != "1" ]]; then
    "${compose[@]}" exec -T postgres sh -c 'createdb -U "$POSTGRES_USER" perpeto_sandbox'
    echo "created database perpeto_sandbox"
  fi
  # The roles and their passwords already exist cluster-wide; this applies the
  # schema ownership and grants inside the sandbox database.
  psql_admin perpeto_sandbox < scripts/bootstrap-db-roles.sql
  docker run --rm --network "$network" -e DATABASE_URL="$(sandbox_url MIGRATE_DATABASE_URL)" \
    --read-only --cap-drop ALL --security-opt no-new-privileges:true \
    --entrypoint funding-arb-migrate "$image"

  local keys
  keys="$(secret KEYS)"
  if [[ -z "$(jq -r '.PERPETO_SECRET_MASTER_KEY // empty' <<<"$keys")" ]]; then
    keys="$(jq --arg master "$(openssl rand -base64 32)" '. + {PERPETO_SECRET_MASTER_KEY: $master}' <<<"$keys")"
    aws secretsmanager put-secret-value --region "$AWS_REGION" \
      --secret-id perpeto/sandbox/KEYS --secret-string file:///dev/stdin <<<"$keys" >/dev/null
    echo "generated the sandbox master key in perpeto/sandbox/KEYS"
  fi
  render_env
  if [[ -z "$(jq -r '.PERPETO_TENANT_ID // empty' <<<"$keys")" ]]; then
    local tenant
    tenant="$(run init funding-arb-venue-admin tenant-ensure "M4 operator" </dev/null | tail -n 1)"
    [[ "$tenant" =~ ^[0-9a-f-]{36}$ ]] || { echo "tenant-ensure failed: $tenant" >&2; exit 1; }
    keys="$(jq --arg tenant "$tenant" '. + {PERPETO_TENANT_ID: $tenant}' <<<"$keys")"
    aws secretsmanager put-secret-value --region "$AWS_REGION" \
      --secret-id perpeto/sandbox/KEYS --secret-string file:///dev/stdin <<<"$keys" >/dev/null
    render_env
    echo "created the M4 operator tenant $tenant"
  fi
  echo "sandbox initialized"
}

# NAME → "VENUE ENVIRONMENT PRODUCT..." for venue-admin import.
targets() {
  case "$1" in
    BYBIT) echo "BYBIT TESTNET SPOT LINEAR_PERPETUAL" ;;
    BINANCE_SPOT) echo "BINANCE TESTNET SPOT" ;;
    BINANCE_PERP) echo "BINANCE TESTNET LINEAR_PERPETUAL" ;;
    OKX) echo "OKX DEMO SPOT LINEAR_PERPETUAL" ;;
    *) echo "unknown sandbox key $1" >&2; exit 2 ;;
  esac
}

import() {
  render_env
  local names=("$@")
  [[ ${#names[@]} -gt 0 ]] || names=(BYBIT BINANCE_SPOT BINANCE_PERP OKX)
  local name venue environment product marker output
  for name in "${names[@]}"; do
    read -r venue environment products <<<"$(targets "$name")"
    if [[ "$(secret "$name" | jq -r '.api_key // empty')" == "" ]]; then
      echo "skipping $name: perpeto/sandbox/$name is empty (run put-sandbox-key.sh $name)"
      continue
    fi
    for product in $products; do
      marker="$state/imported-$venue-$product"
      if [[ -s "$marker" ]]; then
        echo "$venue $product already imported as $(cat "$marker")"
        continue
      fi
      # The key goes from Secrets Manager to the container on stdin only.
      output="$(secret "$name" | run import funding-arb-venue-admin import \
        "$venue" "m4-$(tr 'A-Z_' 'a-z-' <<<"$venue-$product")" "$product" "$environment")"
      echo "$output"
      sed -nE 's/^imported account ([0-9a-f-]{36}) .*/\1/p' <<<"$output" > "$marker"
    done
  done
}

case "${1:-}" in
  init) init ;;
  import) shift; import "$@" ;;
  evidence)
    render_env
    run evidence funding-arb-connector-evidence credentialed </dev/null
    ;;
  open)
    venue_arg "${2:-}"; render_env
    run "open-$2" funding-arb-sandbox open "$2" "${3:?UNDERLYING}" "${4:?NOTIONAL}" </dev/null
    ;;
  close)
    venue_arg "${2:-}"; render_env
    run "close-$2" funding-arb-sandbox close "$2" </dev/null
    ;;
  cycle)
    venue_arg "${2:-}"; render_env
    name="perpeto-sandbox-cycle-${2,,}"
    docker rm "$name" >/dev/null 2>&1 || true
    docker run -d --name "$name" --network "$network" --env-file "$state/sandbox.env" \
      --read-only --cap-drop ALL --security-opt no-new-privileges:true --init \
      --log-driver awslogs --log-opt awslogs-region="$AWS_REGION" \
      --log-opt awslogs-group="$PERPETO_LOG_GROUP" --log-opt tag="sandbox-cycle-$2" \
      --entrypoint funding-arb-sandbox "$image" cycle "$2" "${3:?UNDERLYING}" "${4:?NOTIONAL}" >/dev/null
    echo "started $name; it holds through the next funding settlement. Follow it with: logs $2"
    ;;
  recover)
    venue_arg "${2:-}"; render_env
    fault="${4:-40}"
    [[ "$fault" =~ ^[0-9]+$ ]] || { echo "FAULT_MS must be a number" >&2; exit 2; }
    RUN_EXTRA=(-e "PERPETO_FAULT_SUBMIT_TIMEOUT_MS=$fault")
    run "recover-$2" funding-arb-sandbox recover "$2" "${3:?UNDERLYING}" </dev/null
    ;;
  reconcile)
    render_env
    run reconcile funding-arb-reconciler once </dev/null
    ;;
  report)
    venue_arg "${2:-}"; render_env
    RUN_EXTRA=(-v "$state/evidence:/out")
    run "report-$2" funding-arb-sandbox report "$2" "${3:?SINCE}" /out </dev/null
    cat "$state/evidence/$2.md"
    ;;
  json)
    venue_arg "${2:-}"
    cat "$state/evidence/$2.json"
    ;;
  status)
    docker ps -a --filter name=perpeto-sandbox- --format '{{.Names}}\t{{.Status}}'
    ls "$state" | sed -n 's/^imported-//p'
    ;;
  logs)
    venue_arg "${2:-}"
    docker logs --tail 100 "perpeto-sandbox-cycle-${2,,}" 2>&1
    ;;
  *)
    sed -n '2,15p' "$0" >&2
    exit 2
    ;;
esac
