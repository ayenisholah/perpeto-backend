#!/usr/bin/env bash
# Deploy one image to the AWS staging host (DEC-0020). Run as root through SSM
# Run Command by scripts/aws/deploy-host.sh:
#
#   refresh.sh <git-sha> <image>
#
# It fetches the deployment files from the public repository at exactly that
# commit, renders runtime.env from Secrets Manager (generating the application
# keys once), and runs scripts/deploy-staging.sh with the AWS compose override.
set -euo pipefail

sha="${1:?usage: refresh.sh <git-sha> <image>}"
image="${2:?usage: refresh.sh <git-sha> <image>}"
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "refresh.sh: expected a full commit SHA" >&2; exit 1; }

# shellcheck disable=SC1091
. /etc/perpeto.env
dir=/opt/perpeto
raw="https://raw.githubusercontent.com/${REPOSITORY}/${sha}"

install -d -m 700 "$dir" "$dir/deploy/compose" "$dir/deploy/ec2" "$dir/scripts"
for file in \
  deploy/compose/docker-compose.staging.yml \
  deploy/compose/docker-compose.aws.yml \
  deploy/ec2/Caddyfile \
  deploy/ec2/staging.env \
  scripts/deploy-staging.sh \
  scripts/bootstrap-db-roles.sql; do
  curl -fsSL --retry 3 "$raw/$file" -o "$dir/$file.tmp"
  mv "$dir/$file.tmp" "$dir/$file"
done

secret() {
  aws secretsmanager get-secret-value --region "$AWS_REGION" \
    --secret-id "$SECRET_PREFIX/$1" --query SecretString --output text
}

# Application keys: generated once, kept in Secrets Manager so a rebuilt host
# keeps issuing tokens the clients already hold.
keys="$(secret KEYS)"
if [[ -z "$(jq -r '.PERPETO_ACCESS_TOKEN_KEY // empty' <<<"$keys")" ]]; then
  keys="$(jq -n \
    --arg access "$(openssl rand -base64 32)" \
    --arg mfa "$(openssl rand -base64 32)" \
    --arg master "$(openssl rand -base64 32)" \
    --arg token "$(openssl rand -hex 24)" \
    --arg expires "$(date -u -d '+30 days' +%Y-%m-%dT%H:%M:%SZ)" \
    '{PERPETO_ACCESS_TOKEN_KEY: $access, PERPETO_MFA_KEY: $mfa,
      PERPETO_SECRET_MASTER_KEY: $master, DEPLOYMENT_TOKEN: $token,
      DEPLOYMENT_TOKEN_EXPIRES_AT: $expires}')"
  aws secretsmanager put-secret-value --region "$AWS_REGION" \
    --secret-id "$SECRET_PREFIX/KEYS" --secret-string "$keys" >/dev/null
  echo "generated the application keys and a one-use deployment token in $SECRET_PREFIX/KEYS"
fi
key() { jq -r --arg name "$1" '.[$name]' <<<"$keys"; }

postgres_password="$(secret POSTGRES_PASSWORD)"
migrator_password="$(secret MIGRATOR_PASSWORD)"
app_password="$(secret APP_PASSWORD)"
token_hash="$(printf '%s' "$(key DEPLOYMENT_TOKEN)" | openssl dgst -sha256 -hex | awk '{print $NF}')"

umask 077
{
  cat "$dir/deploy/ec2/staging.env"
  echo
  echo "# --- rendered by deploy/ec2/refresh.sh from Secrets Manager; do not edit ---"
  echo "POSTGRES_PASSWORD=${postgres_password}"
  echo "MIGRATE_DATABASE_URL=postgres://perpeto_migrator:${migrator_password}@postgres:5432/perpeto"
  echo "DATABASE_URL=postgres://perpeto_app:${app_password}@postgres:5432/perpeto"
  echo "PERPETO_ACCESS_TOKEN_KEY=$(key PERPETO_ACCESS_TOKEN_KEY)"
  echo "PERPETO_MFA_KEY=$(key PERPETO_MFA_KEY)"
  echo "PERPETO_SECRET_MASTER_KEY=$(key PERPETO_SECRET_MASTER_KEY)"
  echo "PERPETO_BOOTSTRAP_TOKEN_HASH=${token_hash}"
  echo "PERPETO_BOOTSTRAP_TOKEN_EXPIRES_AT=$(key DEPLOYMENT_TOKEN_EXPIRES_AT)"
  # Read by Compose for the AWS override (Caddy hosts and the awslogs driver).
  echo "AWS_REGION=${AWS_REGION}"
  echo "PERPETO_LOG_GROUP=${PERPETO_LOG_GROUP}"
  echo "PERPETO_PUBLIC_HOSTS=${PERPETO_PUBLIC_HOSTS}"
} > "$dir/runtime.env.tmp"
mv "$dir/runtime.env.tmp" "$dir/runtime.env"

cd "$dir"
PERPETO_IMAGE="$image" \
PERPETO_COMPOSE_OVERRIDE=deploy/compose/docker-compose.aws.yml \
PERPETO_PROVISION_DB_ROLES=1 \
  bash scripts/deploy-staging.sh
printf '%s\n' "$sha" > .current-sha
