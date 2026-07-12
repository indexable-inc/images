---
paths: "**/Pulumi.yaml, **/Pulumi.*.yaml"
---

# Pulumi Infrastructure as Code

## Project Structure

```
my-project/
├── Pulumi.yaml          # Project definition
├── Pulumi.dev.yaml      # Stack config (dev)
├── Pulumi.prod.yaml     # Stack config (prod)
├── index.ts             # TypeScript entry point
└── package.json
```

## State Backend (IMPORTANT)

**Never use Pulumi Cloud.** Use S3 or local for state storage.

```bash
# Option 1: S3 backend (recommended - backed up, shareable)
export AWS_ACCESS_KEY_ID=<s3-access-key>
export AWS_SECRET_ACCESS_KEY=<s3-secret-key>
export AWS_ENDPOINT_URL=https://fsn1.your-objectstorage.com  # Hetzner
pulumi login s3://pulumi-state

# Option 2: Local (simple, not backed up)
pulumi login --local
```

State is just JSON files tracking what resources exist. S3 backend needs no service - just bucket read/write access.

## Common Commands

```bash
# Create new project
pulumi new typescript

# Select/create stack
pulumi stack select dev
pulumi stack init prod

# Preview and deploy
pulumi preview
pulumi up

# View outputs
pulumi stack output

# Destroy resources
pulumi destroy
```

## TypeScript Patterns

```typescript
import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

// Config
const config = new pulumi.Config();
const instanceType = config.get("instanceType") || "t3.micro";

// Resources
const bucket = new aws.s3.Bucket("my-bucket", {
  acl: "private",
  tags: { Environment: pulumi.getStack() },
});

// Exports
export const bucketName = bucket.id;
export const bucketArn = bucket.arn;
```

## Stack References (Cross-Stack)

```typescript
const networkStack = new pulumi.StackReference("org/network/prod");
const vpcId = networkStack.getOutput("vpcId");
```

## Secrets

```bash
# Set secret
pulumi config set --secret dbPassword hunter2

# In code
const dbPassword = config.requireSecret("dbPassword");
```

## Reference Documentation

Full Pulumi docs: `~/.config/nix/claude/global/skills/pulumi/docs/content/`

- `docs/content/docs/iac/` - Infrastructure as Code concepts
- `docs/content/docs/get-started/` - Getting started guides
- `docs/content/docs/reference/` - CLI and API reference
- `docs/content/docs/esc/` - Environments, Secrets, Configuration
