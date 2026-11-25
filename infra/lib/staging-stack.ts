import {
  CfnOutput,
  Duration,
  RemovalPolicy,
  SecretValue,
  Stack,
  Tags,
  Validations,
  type StackProps,
} from "aws-cdk-lib";
import * as backup from "aws-cdk-lib/aws-backup";
import * as cloudwatch from "aws-cdk-lib/aws-cloudwatch";
import * as cwActions from "aws-cdk-lib/aws-cloudwatch-actions";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import * as events from "aws-cdk-lib/aws-events";
import * as iam from "aws-cdk-lib/aws-iam";
import * as logs from "aws-cdk-lib/aws-logs";
import * as secretsmanager from "aws-cdk-lib/aws-secretsmanager";
import type { Construct } from "constructs";
import { config, DATABASE_SECRETS, KEYS_SECRET, SANDBOX_PREFIX, SANDBOX_SECRETS } from "./config";

/**
 * The PAPER staging host: one EC2 instance running the existing
 * docker-compose stack (PostgreSQL, Redis, the Perpeto services) behind Caddy
 * for TLS. Stateful, so the stack has termination protection, the instance
 * has API termination disabled, and EBS is backed up daily.
 *
 * There is no SSH: the deploy workflow and the operator reach the host through
 * SSM (Run Command and Session Manager). Only 80 and 443 are open.
 */
export class StagingStack extends Stack {
  readonly instance: ec2.Instance;

  constructor(scope: Construct, id: string, props: StackProps) {
    super(scope, id, { ...props, terminationProtection: true });

    // A dedicated public-only VPC: no NAT gateway (no cost), and no lookup of
    // account-specific context, so `cdk synth` runs in CI without credentials.
    const vpc = new ec2.Vpc(this, "Vpc", {
      maxAzs: 1,
      natGateways: 0,
      subnetConfiguration: [{ name: "public", subnetType: ec2.SubnetType.PUBLIC, cidrMask: 24 }],
    });

    const databaseSecrets = DATABASE_SECRETS.map(
      (name) =>
        new secretsmanager.Secret(this, `Secret${name}`, {
          secretName: `${config.secretPrefix}/${name}`,
          description: `Perpeto staging ${name} (generated).`,
          generateSecretString: { passwordLength: 48, excludePunctuation: true },
          removalPolicy: RemovalPolicy.RETAIN,
        }),
    );
    const keys = new secretsmanager.Secret(this, "SecretKeys", {
      secretName: `${config.secretPrefix}/${KEYS_SECRET}`,
      description: "Perpeto staging application keys and deployment token (written by the host on first deploy).",
      secretStringValue: SecretValue.unsafePlainText("{}"),
      removalPolicy: RemovalPolicy.RETAIN,
    });

    // M4 SANDBOX credentials (testnet and demo only). Filled by the operator
    // from their own terminal; the host reads them to import accounts into the
    // separate sandbox database, and writes only the sandbox KEYS secret.
    const sandboxSecrets = SANDBOX_SECRETS.map(
      (name) =>
        new secretsmanager.Secret(this, `Sandbox${name}`, {
          secretName: `${SANDBOX_PREFIX}/${name}`,
          description: `Perpeto M4 sandbox ${name} (testnet/demo only).`,
          secretStringValue: SecretValue.unsafePlainText("{}"),
          removalPolicy: RemovalPolicy.RETAIN,
        }),
    );

    const logGroup = new logs.LogGroup(this, "Logs", {
      logGroupName: config.logGroup,
      retention: logs.RetentionDays.ONE_MONTH,
      removalPolicy: RemovalPolicy.RETAIN,
    });

    const sg = new ec2.SecurityGroup(this, "WebSg", {
      vpc,
      description: "Perpeto staging: HTTP/HTTPS only (Caddy). No SSH; use SSM.",
      allowAllOutbound: true,
    });
    sg.addIngressRule(ec2.Peer.anyIpv4(), ec2.Port.tcp(80), "HTTP (ACME challenge and redirect)");
    sg.addIngressRule(ec2.Peer.anyIpv4(), ec2.Port.tcp(443), "HTTPS API");

    const role = new iam.Role(this, "InstanceRole", {
      assumedBy: new iam.ServicePrincipal("ec2.amazonaws.com"),
      description: "Perpeto staging host",
      managedPolicies: [iam.ManagedPolicy.fromAwsManagedPolicyName("AmazonSSMManagedInstanceCore")],
    });
    logGroup.grantWrite(role);
    for (const secret of databaseSecrets) secret.grantRead(role);
    keys.grantRead(role);
    keys.grantWrite(role);
    for (const secret of sandboxSecrets) secret.grantRead(role);
    sandboxSecrets[SANDBOX_SECRETS.indexOf("KEYS")].grantWrite(role);

    const eip = new ec2.CfnEIP(this, "Eip", { domain: "vpc", tags: [{ key: "Name", value: "perpeto-staging" }] });

    // First boot only installs Docker and records the settings refresh.sh
    // reads. The application is deployed by the workflow through SSM, because
    // only it knows which image to run.
    const userData = ec2.UserData.forLinux();
    userData.addCommands(
      "set -euxo pipefail",
      "dnf install -y docker jq",
      "mkdir -p /usr/local/lib/docker/cli-plugins",
      'curl -fsSL "https://github.com/docker/compose/releases/download/v2.39.4/docker-compose-linux-x86_64" -o /usr/local/lib/docker/cli-plugins/docker-compose',
      "chmod +x /usr/local/lib/docker/cli-plugins/docker-compose",
      "systemctl enable --now docker",
      "install -d -m 700 /opt/perpeto",
      `cat > /etc/perpeto.env <<ENV
AWS_REGION=${this.region}
SECRET_PREFIX=${config.secretPrefix}
PERPETO_LOG_GROUP=${config.logGroup}
REPOSITORY=${config.repository}
PERPETO_PUBLIC_HOSTS="${config.domain}, ${eip.attrPublicIp}.sslip.io"
ENV`,
      "chmod 600 /etc/perpeto.env",
    );

    // The ID is unique in the account on purpose: requireImdsv2 creates a
    // launch template named after it, and launch template names are
    // account-wide ("Host" is already taken by another stack).
    this.instance = new ec2.Instance(this, "PerpetoHost", {
      vpc,
      vpcSubnets: { subnetType: ec2.SubnetType.PUBLIC },
      instanceType: new ec2.InstanceType(config.instanceType),
      creditSpecification: ec2.CpuCredits.STANDARD,
      // Pinned (config.amiId), never resolved at deploy time: a newer AMI would
      // replace the stateful host.
      machineImage: ec2.MachineImage.genericLinux({ [config.region]: config.amiId }),
      securityGroup: sg,
      role,
      userData,
      requireImdsv2: true,
      detailedMonitoring: false,
      blockDevices: [
        {
          deviceName: "/dev/xvda",
          volume: ec2.BlockDeviceVolume.ebs(config.rootVolumeGiB, {
            volumeType: ec2.EbsDeviceVolumeType.GP3,
            encrypted: true,
          }),
        },
      ],
      instanceName: "perpeto-staging",
    });
    (this.instance.node.defaultChild as ec2.CfnInstance).disableApiTermination = true;
    Validations.of(this.instance).acknowledge({
      id: "CloudFormation-Validate::W9010",
      reason: "The AMI is pinned on purpose (config.amiId) so a new image never replaces the stateful host.",
    });
    // Containers never call AWS: the awslogs driver uses dockerd's host
    // credentials, so the IMDS hop limit stays at one.
    new ec2.CfnEIPAssociation(this, "EipAssociation", {
      allocationId: eip.attrAllocationId,
      instanceId: this.instance.instanceId,
    });

    new cloudwatch.Alarm(this, "AutoRecover", {
      alarmName: "perpeto-staging-host-auto-recover",
      alarmDescription: "System status check failed: EC2 recovers the instance onto healthy hardware.",
      metric: new cloudwatch.Metric({
        namespace: "AWS/EC2",
        metricName: "StatusCheckFailed_System",
        dimensionsMap: { InstanceId: this.instance.instanceId },
        statistic: "Maximum",
        period: Duration.minutes(1),
      }),
      threshold: 0,
      comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_THRESHOLD,
      evaluationPeriods: 2,
      datapointsToAlarm: 2,
      treatMissingData: cloudwatch.TreatMissingData.MISSING,
    }).addAlarmAction(new cwActions.Ec2Action(cwActions.Ec2InstanceAction.RECOVER));

    const vault = new backup.BackupVault(this, "Vault", {
      backupVaultName: "perpeto-staging",
      removalPolicy: RemovalPolicy.RETAIN,
    });
    const plan = new backup.BackupPlan(this, "Plan", {
      backupPlanName: "perpeto-staging-daily",
      backupVault: vault,
      backupPlanRules: [
        new backup.BackupPlanRule({
          ruleName: "daily-7d",
          scheduleExpression: events.Schedule.cron({ hour: "3", minute: "15" }),
          deleteAfter: Duration.days(7),
        }),
      ],
    });
    plan.addSelection("Host", { resources: [backup.BackupResource.fromEc2Instance(this.instance)] });

    Tags.of(this).add("project", config.tags.project);

    new CfnOutput(this, "InstanceId", { value: this.instance.instanceId });
    new CfnOutput(this, "PublicIp", { value: eip.attrPublicIp });
    new CfnOutput(this, "SslipUrl", { value: `https://${eip.attrPublicIp}.sslip.io` });
    new CfnOutput(this, "ApiUrl", { value: `https://${config.domain}` });
    new CfnOutput(this, "SecretPrefix", { value: config.secretPrefix });
  }
}
