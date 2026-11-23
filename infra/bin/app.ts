#!/usr/bin/env node
// Perpeto PAPER staging on AWS (DEC-0020). See docs/DEPLOYMENT.md.
import { App, Tags } from "aws-cdk-lib";
import { config } from "../lib/config";
import { DeployerStack } from "../lib/deployer-stack";
import { StagingStack } from "../lib/staging-stack";

const app = new App();
// The account comes from the deploying credentials, so none is committed.
const env = { account: process.env.CDK_DEFAULT_ACCOUNT, region: config.region };
for (const [key, value] of Object.entries(config.tags)) Tags.of(app).add(key, value);

const staging = new StagingStack(app, "Perpeto-Staging", {
  env,
  description: "Perpeto PAPER staging host (docker compose behind Caddy)",
});
new DeployerStack(app, "Perpeto-Deployer", {
  env,
  description: "IAM identity the Deploy Staging workflow uses",
  instanceId: staging.instance.instanceId,
});
