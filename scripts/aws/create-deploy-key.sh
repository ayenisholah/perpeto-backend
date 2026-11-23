#!/usr/bin/env bash
# Create (or rotate) the access key of the perpeto-github-deployer IAM user and
# store it in the GitHub `staging` environment (DEC-0020). The key goes
# straight from AWS into GitHub and is never printed. Older keys are deleted
# after the new one is stored.
#
# Needs AWS credentials that can manage the user's keys, and `gh` logged in
# with admin rights on the repository.
set -euo pipefail

user=perpeto-github-deployer
region="${AWS_REGION:-eu-north-1}"
repository="${GITHUB_REPOSITORY:-ayenisholah/perpeto-backend}"

old_keys="$(aws iam list-access-keys --user-name "$user" --query 'AccessKeyMetadata[].AccessKeyId' --output text)"
if [[ "$(wc -w <<<"$old_keys")" -ge 2 ]]; then
  echo "$user already has two access keys; delete one first" >&2
  exit 1
fi

key="$(aws iam create-access-key --user-name "$user" --output json)"
jq -r '.AccessKey.AccessKeyId' <<<"$key" | gh secret set AWS_ACCESS_KEY_ID --env staging --repo "$repository"
jq -r '.AccessKey.SecretAccessKey' <<<"$key" | gh secret set AWS_SECRET_ACCESS_KEY --env staging --repo "$repository"
gh variable set AWS_REGION --env staging --repo "$repository" --body "$region"
new_id="$(jq -r '.AccessKey.AccessKeyId' <<<"$key")"
unset key

for old in $old_keys; do
  aws iam delete-access-key --user-name "$user" --access-key-id "$old"
  echo "deleted old key ${old:0:8}..."
done
echo "stored key ${new_id:0:8}... in the staging environment of $repository"
