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
  deploy/ec2/update-web.sh \
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
# The web platform's session-cookie key (DEC-0021), added once to existing keys.
if [[ -z "$(jq -r '.PERPETO_WEB_SESSION_KEY // empty' <<<"$keys")" ]]; then
  keys="$(jq --arg web "$(openssl rand -base64 32)" '. + {PERPETO_WEB_SESSION_KEY: $web}' <<<"$keys")"
  aws secretsmanager put-secret-value --region "$AWS_REGION" \
    --secret-id "$SECRET_PREFIX/KEYS" --secret-string "$keys" >/dev/null
  echo "generated the web session key in $SECRET_PREFIX/KEYS"
fi
key() { jq -r --arg name "$1" '.[$name]' <<<"$keys"; }

# The web demo answers on PERPETO_APP_DOMAIN and on app.<elastic-ip>.sslip.io,
# which works before the DNS record exists.
app_domain="$(sed -n 's/^PERPETO_APP_DOMAIN=//p' "$dir/deploy/ec2/staging.env" | tail -n 1)"
sslip_host="$(tr ',' '\n' <<<"$PERPETO_PUBLIC_HOSTS" | tr -d ' ' | grep 'sslip\.io$' | head -n 1)"
app_hosts="${app_domain:-app.perpeto.xyz}${sslip_host:+, app.${sslip_host}}"

# The web image is published by perpeto-web. Keep the one update-web.sh last
# deployed, else follow main; enable the web profile only when the image can
# actually be pulled.
web_image="$(cat "$dir/.current-web-image" 2>/dev/null || echo "ghcr.io/ayenisholah/perpeto-web:main")"
profiles=""
if docker pull --quiet "$web_image" >/dev/null 2>&1; then
  profiles="web"
else
  echo "web image $web_image is not available yet; deploying without the web demo" >&2
fi

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
  echo "PERPETO_APP_HOSTS=${app_hosts}"
  echo "PERPETO_WEB_IMAGE=${web_image}"
} > "$dir/runtime.env.tmp"
mv "$dir/runtime.env.tmp" "$dir/runtime.env"

# The web server's own environment: only what it needs, never database or API
# signing keys. It reaches the API over the compose network.
first_app_host="${app_hosts%%,*}"
{
  echo "PERPETO_API_URL=http://api:8080"
  echo "PERPETO_WEB_ORIGIN=https://${first_app_host}"
  echo "PERPETO_WEB_SESSION_KEY=$(key PERPETO_WEB_SESSION_KEY)"
  sed -n 's/^PERPETO_GOOGLE_WEB_CLIENT_ID=/NEXT_PUBLIC_GOOGLE_CLIENT_ID=/p' "$dir/deploy/ec2/staging.env"
  sed -n 's/^PERPETO_APPLE_WEB_SERVICES_ID=/NEXT_PUBLIC_APPLE_SERVICES_ID=/p' "$dir/deploy/ec2/staging.env"
} > "$dir/web.env.tmp"
mv "$dir/web.env.tmp" "$dir/web.env"

cd "$dir"
COMPOSE_PROFILES="$profiles" \
PERPETO_IMAGE="$image" \
PERPETO_COMPOSE_OVERRIDE=deploy/compose/docker-compose.aws.yml \
PERPETO_PROVISION_DB_ROLES=1 \
  bash scripts/deploy-staging.sh
chmod 700 deploy/ec2/update-web.sh
printf '%s\n' "$sha" > .current-sha
