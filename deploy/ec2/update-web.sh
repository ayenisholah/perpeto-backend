#!/usr/bin/env bash
# Roll the hosted web demo to one image (DEC-0021). Run as root through SSM Run
# Command by perpeto-web's deploy workflow, after a backend deploy has put
# this file on the host:
#
#   update-web.sh ghcr.io/ayenisholah/perpeto-web:sha-<git-sha>
#
# Only the `web` container is recreated; the backend services are untouched.
set -euo pipefail

image="${1:?usage: update-web.sh <image>}"
[[ "$image" =~ ^ghcr\.io/ayenisholah/perpeto-web:sha-[0-9a-f]{40}$ ]] || {
  echo "update-web.sh: expected ghcr.io/ayenisholah/perpeto-web:sha-<full sha>" >&2
  exit 1
}

dir=/opt/perpeto
cd "$dir"
[[ -f runtime.env && -f web.env && -f .current-image ]] || {
  echo "update-web.sh: the backend stack has not been deployed on this host" >&2
  exit 1
}

docker pull --quiet "$image" >/dev/null
sed -i "s|^PERPETO_WEB_IMAGE=.*|PERPETO_WEB_IMAGE=${image}|" runtime.env
COMPOSE_PROFILES=web PERPETO_IMAGE="$(cat .current-image)" docker compose --env-file runtime.env \
  -f deploy/compose/docker-compose.staging.yml -f deploy/compose/docker-compose.aws.yml \
  up -d --no-deps --wait --wait-timeout 60 web
# Later backend deploys keep this image rather than falling back to main.
printf '%s\n' "$image" > .current-web-image
echo "web demo is running $image"
