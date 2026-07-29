#!/usr/bin/env bash
# Run the API against the local development stack, reachable from a phone.
#
#   docker compose -f deploy/compose/docker-compose.dev.yml up -d
#   bash scripts/dev-run-api.sh
#
# Secrets are generated once into deploy/dev.env (gitignored) and reused, so the
# access tokens minted across restarts stay valid. Delete that file to rotate.
#
# The API binds 0.0.0.0:8080 so a phone on the same network can reach it; the
# script prints the LAN URL to put in perpeto-mobile/.env.local as
# EXPO_PUBLIC_API_URL. PAPER mode is forced: this path must never carry
# production credentials or reach a real venue.
set -euo pipefail

cd "$(dirname "$0")/.."
env_file="deploy/dev.env"

if [[ ! -f "$env_file" ]]; then
  command -v openssl >/dev/null 2>&1 || { echo "openssl is required" >&2; exit 1; }
  token="$(openssl rand -hex 24)"
  cat > "$env_file" <<ENV
PERPETO_MODE=PAPER
PERPETO_API_BIND=0.0.0.0:8080
DATABASE_URL=postgres://perpeto_app:perpeto_app@127.0.0.1:5432/perpeto
MIGRATE_DATABASE_URL=postgres://perpeto_migrator:perpeto_migrator@127.0.0.1:5432/perpeto
PERPETO_ACCESS_TOKEN_KEY=$(openssl rand -base64 32)
PERPETO_MFA_KEY=$(openssl rand -base64 32)
PERPETO_TOKEN_ISSUER=http://localhost:8080
PERPETO_BOOTSTRAP_TOKEN_HASH=$(printf '%s' "$token" | openssl dgst -sha256 -hex | awk '{print $NF}')
PERPETO_BOOTSTRAP_TOKEN_EXPIRES_AT=$(date -u -d '+30 days' +%Y-%m-%dT%H:%M:%SZ)
# Local development only. DEC-0012 forbids this authorization model for the M4
# pilot, so it must never be set on a shared or deployed environment.
PERPETO_OPEN_REGISTRATION=true
# Keep offline; enter once in the app to seat the first Owner.
DEPLOYMENT_TOKEN=$token
ENV
  echo "generated $env_file (deployment token: $token)"
fi

set -a
# shellcheck disable=SC1090
. "./$env_file"
set +a

DATABASE_URL="$MIGRATE_DATABASE_URL" cargo run --locked --bin funding-arb-migrate

address="$(ip -4 addr show scope global 2>/dev/null \
  | grep -oP 'inet \K[\d.]+' | grep -v '^172\.1[78]\.' | head -1 || true)"
echo "API starting on http://${address:-127.0.0.1}:8080"
echo "set EXPO_PUBLIC_API_URL=http://${address:-127.0.0.1}:8080 in perpeto-mobile/.env.local"

exec cargo run --locked --bin funding-arb-api
