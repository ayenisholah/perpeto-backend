#!/usr/bin/env bash
# Create (or rotate) a deployer IAM user's access key and store it in a GitHub
# environment (DEC-0020, DEC-0021). The key goes straight from AWS into GitHub
# and is never printed. Older keys are deleted after the new one is stored.
#
#   create-deploy-key.sh                      # backend: perpeto-github-deployer -> staging
#   create-deploy-key.sh perpeto-web-deployer ayenisholah/perpeto-web staging
#
# Needs AWS credentials that can manage the user's keys, and `gh` logged in
# with admin rights on the repository.
set -euo pipefail

user="${1:-perpeto-github-deployer}"
repository="${2:-${GITHUB_REPOSITORY:-ayenisholah/perpeto-backend}}"
environment="${3:-staging}"
region="${AWS_REGION:-eu-north-1}"

old_keys="$(aws iam list-access-keys --user-name "$user" --query 'AccessKeyMetadata[].AccessKeyId' --output text)"
if [[ "$(wc -w <<<"$old_keys")" -ge 2 ]]; then
  echo "$user already has two access keys; delete one first" >&2
  exit 1
fi

# The environment must exist before it can hold secrets; creating it is idempotent.
gh api --method PUT "repos/$repository/environments/$environment" >/dev/null

key="$(aws iam create-access-key --user-name "$user" --output json)"
jq -r '.AccessKey.AccessKeyId' <<<"$key" | gh secret set AWS_ACCESS_KEY_ID --env "$environment" --repo "$repository"
jq -r '.AccessKey.SecretAccessKey' <<<"$key" | gh secret set AWS_SECRET_ACCESS_KEY --env "$environment" --repo "$repository"
gh variable set AWS_REGION --env "$environment" --repo "$repository" --body "$region"
new_id="$(jq -r '.AccessKey.AccessKeyId' <<<"$key")"
unset key

for old in $old_keys; do
  aws iam delete-access-key --user-name "$user" --access-key-id "$old"
  echo "deleted old key ${old:0:8}..."
done
echo "stored key ${new_id:0:8}... in the $environment environment of $repository"
