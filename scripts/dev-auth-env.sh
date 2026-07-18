#!/usr/bin/env bash
# Generate the authentication secrets required by runtime.env.
# Prints ready-to-paste values plus the plaintext deployment token (store it
# offline; only its hash goes in runtime.env).
set -euo pipefail

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl is required" >&2
  exit 1
fi

access_key="$(openssl rand -base64 32)"
mfa_key="$(openssl rand -base64 32)"
deploy_token="$(openssl rand -hex 24)"
deploy_hash="$(printf '%s' "$deploy_token" | openssl dgst -sha256 -hex | awk '{print $NF}')"
expiry="$(date -u -d '+30 days' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v+30d +%Y-%m-%dT%H:%M:%SZ)"

cat <<ENV
# --- paste into runtime.env ---
SIGNEX_ACCESS_TOKEN_KEY=${access_key}
SIGNEX_MFA_KEY=${mfa_key}
SIGNEX_BOOTSTRAP_TOKEN_HASH=${deploy_hash}
SIGNEX_BOOTSTRAP_TOKEN_EXPIRES_AT=${expiry}

# --- keep this offline; enter it once in the app to seat the first Owner ---
DEPLOYMENT_TOKEN=${deploy_token}
ENV
