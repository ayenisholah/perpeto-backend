import { CfnOutput, Stack, Tags, type StackProps } from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import type { Construct } from "constructs";
import { config } from "./config";

export type DeployerStackProps = StackProps & { instanceId: string };

/**
 * The identity the Deploy Staging workflow uses. The AWS organisation's
 * service control policy forbids creating an OIDC provider, so this is an IAM
 * user. It can only assume the CDK bootstrap roles (which do the deploying),
 * read the Perpeto stack outputs, and run commands on the staging host through
 * SSM. Its access key lives in the GitHub `staging` environment; create or
 * rotate it with scripts/aws/create-deploy-key.sh.
 */
export class DeployerStack extends Stack {
  constructor(scope: Construct, id: string, props: DeployerStackProps) {
    super(scope, id, props);
    const user = new iam.User(this, "GithubDeployer", { userName: "perpeto-github-deployer" });
    const a = this.account;
    const r = this.region;
    user.addToPolicy(
      new iam.PolicyStatement({
        sid: "AssumeCdkBootstrapRoles",
        actions: ["sts:AssumeRole"],
        resources: [`arn:aws:iam::${a}:role/cdk-hnb659fds-*-${a}-${r}`],
      }),
    );
    user.addToPolicy(
      new iam.PolicyStatement({
        sid: "ReadStacks",
        actions: ["cloudformation:DescribeStacks"],
        resources: [`arn:aws:cloudformation:${r}:${a}:stack/Perpeto-*/*`],
      }),
    );
    user.addToPolicy(
      new iam.PolicyStatement({
        sid: "DeployToHost",
        actions: ["ssm:SendCommand"],
        resources: [
          `arn:aws:ec2:${r}:${a}:instance/${props.instanceId}`,
          `arn:aws:ssm:${r}::document/AWS-RunShellScript`,
        ],
      }),
    );
    user.addToPolicy(
      new iam.PolicyStatement({
        sid: "ReadCommandResults",
        actions: ["ssm:GetCommandInvocation", "ssm:ListCommandInvocations"],
        resources: ["*"],
      }),
    );
    Tags.of(this).add("project", config.tags.project);
    new CfnOutput(this, "DeployerUserName", { value: user.userName });
  }
}
