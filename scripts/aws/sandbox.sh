#!/usr/bin/env bash
# Run one M4 sandbox command on the staging host through SSM Run Command, and
# print its output. The host side is deploy/ec2/sandbox.sh; see it for the
# commands. Needs AWS credentials that can run commands on the host.
#
#   scripts/aws/sandbox.sh init
#   scripts/aws/sandbox.sh import
#   scripts/aws/sandbox.sh cycle BYBIT ETH 150
#   scripts/aws/sandbox.sh report BYBIT 2026-11-26T00:00:00Z > docs/evidence/m4/BYBIT.md
set -euo pipefail

[[ $# -ge 1 ]] || { echo "usage: $0 COMMAND [ARGS...] (see deploy/ec2/sandbox.sh)" >&2; exit 2; }
region="${AWS_REGION:-eu-north-1}"
# Arguments travel inside a shell command on the host, so allow only plain tokens.
for argument in "$@"; do
  [[ "$argument" =~ ^[A-Za-z0-9:._+-]+$ ]] || { echo "unsafe argument: $argument" >&2; exit 2; }
done

instance_id="$(aws cloudformation describe-stacks --region "$region" --stack-name Perpeto-Staging \
  --query "Stacks[0].Outputs[?OutputKey=='InstanceId'].OutputValue" --output text)"
parameters="$(jq -n --arg command "bash /opt/perpeto/deploy/ec2/sandbox.sh $*" '{
  commands: ["set -euo pipefail", $command],
  executionTimeout: ["1800"]
}')"
command_id="$(aws ssm send-command --region "$region" \
  --instance-ids "$instance_id" \
  --document-name AWS-RunShellScript \
  --comment "perpeto sandbox $1" \
  --parameters "$parameters" \
  --query Command.CommandId --output text)"
echo "SSM command $command_id: sandbox $*" >&2

status=Pending
for _ in $(seq 1 190); do
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
  --query StandardOutputContent --output text
errors="$(aws ssm get-command-invocation --region "$region" \
  --command-id "$command_id" --instance-id "$instance_id" \
  --query StandardErrorContent --output text)"
[[ -z "$errors" || "$errors" == "None" ]] || echo "$errors" >&2
[[ "$status" == "Success" ]] || { echo "sandbox $1 failed: SSM status $status" >&2; exit 1; }
