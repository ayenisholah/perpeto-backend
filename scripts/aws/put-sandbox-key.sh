#!/usr/bin/env bash
# Store one M4 sandbox (testnet or demo) API key in AWS Secrets Manager.
#
#   put-sandbox-key.sh BYBIT          # Bybit testnet unified account (spot + linear)
#   put-sandbox-key.sh BINANCE_SPOT   # testnet.binance.vision
#   put-sandbox-key.sh BINANCE_PERP   # testnet.binancefuture.com (USDⓈ-M)
#   put-sandbox-key.sh OKX            # OKX demo trading (key, secret, passphrase)
#
# Run it yourself, in your own terminal. It reads the values without echoing
# them, never puts them in argv or a file, and sends them to Secrets Manager
# on stdin. Only testnet and demo keys belong here: give them trade permission
# and never withdrawal permission. Needs AWS credentials that can write
# perpeto/sandbox/* in eu-north-1.
set -euo pipefail

name="${1:-}"
case "$name" in
  BYBIT | BINANCE_SPOT | BINANCE_PERP | OKX) ;;
  *)
    echo "usage: $0 BYBIT|BINANCE_SPOT|BINANCE_PERP|OKX" >&2
    exit 2
    ;;
esac
region="${AWS_REGION:-eu-north-1}"

read -r -s -p "$name API key: " api_key && echo
read -r -s -p "$name API secret: " api_secret && echo
passphrase=""
if [[ "$name" == OKX ]]; then
  read -r -s -p "$name API passphrase: " passphrase && echo
fi
[[ -n "$api_key" && -n "$api_secret" ]] || { echo "key and secret are required" >&2; exit 1; }
[[ "$name" != OKX || -n "$passphrase" ]] || { echo "OKX needs its passphrase" >&2; exit 1; }

# jq reads the values from its environment, so they never appear in argv.
API_KEY="$api_key" API_SECRET="$api_secret" PASSPHRASE="$passphrase" \
  jq -n '{api_key: env.API_KEY, api_secret: env.API_SECRET}
         + (if env.PASSPHRASE == "" then {} else {passphrase: env.PASSPHRASE} end)' |
  aws secretsmanager put-secret-value --region "$region" \
    --secret-id "perpeto/sandbox/$name" --secret-string file:///dev/stdin >/dev/null
echo "stored perpeto/sandbox/$name (key ending ${api_key: -4})"
unset api_key api_secret passphrase
