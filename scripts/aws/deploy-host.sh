#!/usr/bin/env bash
# Deploy an image to the AWS staging host through SSM Run Command (DEC-0020).
# Used by .github/workflows/DEPLOY_STAGING.yml; works the same from a shell
# with AWS credentials that can run commands on the host.
#
#   scripts/aws/deploy-host.sh <git-sha> <image>
set -euo pipefail

sha="${1:?usage: deploy-host.sh <git-sha> <image>}"
image="${2:?usage: deploy-host.sh <git-sha> <image>}"
region="${AWS_REGION:-eu-north-1}"
repository="${GITHUB_REPOSITORY:-ayenisholah/perpeto-backend}"
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "expected a full commit SHA" >&2; exit 1; }
[[ "$image" =~ ^[a-z0-9./_-]+:[A-Za-z0-9._-]+$ ]] || { echo "unexpected image reference: $image" >&2; exit 1; }

output() {
  aws cloudformation describe-stacks --region "$region" --stack-name Perpeto-Staging \
    --query "Stacks[0].Outputs[?OutputKey=='$1'].OutputValue" --output text
}
instance_id="$(output InstanceId)"
sslip_url="$(output SslipUrl)"

script="https://raw.githubusercontent.com/${repository}/${sha}/deploy/ec2/refresh.sh"
parameters="$(jq -n --arg script "$script" --arg sha "$sha" --arg image "$image" '{
  commands: [
    "set -euo pipefail",
    ("curl -fsSL --retry 3 " + $script + " -o /opt/perpeto/refresh.sh"),
    ("bash /opt/perpeto/refresh.sh " + $sha + " " + $image)
  ],
  executionTimeout: ["1200"]
}')"

command_id="$(aws ssm send-command --region "$region" \
  --instance-ids "$instance_id" \
  --document-name AWS-RunShellScript \
  --comment "perpeto staging ${sha:0:12}" \
  --parameters "$parameters" \
  --query Command.CommandId --output text)"
echo "SSM command $command_id on $instance_id"

status=Pending
for _ in $(seq 1 130); do
  sleep 10
  status="$(aws ssm get-command-invocation --region "$region" \
    --command-id "$command_id" --instance-id "$instance_id" \
    --query Status --output text 2>/dev/null || echo Pending)"
  case "$status" in
    Pending | InProgress | Delayed) continue ;;
    *) break ;;
  esac
done

aws ssm get-command-invocation --region "$region" \
  --command-id "$command_id" --instance-id "$instance_id" \
  --query '[StandardOutputContent, StandardErrorContent]' --output text || true

if [[ "$status" != "Success" ]]; then
  echo "deployment failed: SSM status $status" >&2
  exit 1
fi

# Public readiness through Caddy. The sslip.io name works before the
# api.perpeto.xyz DNS record points at the Elastic IP.
for _ in $(seq 1 30); do
  if curl --fail --silent --max-time 5 "$sslip_url/readyz" >/dev/null; then
    echo "staging is ready at $sslip_url"
    exit 0
  fi
  sleep 5
done
echo "the host deployed, but $sslip_url/readyz is not answering" >&2
exit 1
