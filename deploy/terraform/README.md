# VEIL Runtime — Terraform Deployment

Deploy the VEIL ProductHost to your AWS account using ECS Fargate.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ AWS Account                                                      │
│                                                                   │
│  Route53 ──→ ALB (HTTPS) ──→ ECS Fargate (veil-runtime)         │
│                                    │                              │
│                              ┌─────┴─────┐                       │
│                              │           │                        │
│                           S3 Bucket   DynamoDB                    │
│                          (git/projects)  (metadata)               │
│                                                                   │
│  ECR (container images)    CloudWatch (logs)                      │
└─────────────────────────────────────────────────────────────────┘
```

## Prerequisites

1. An AWS account with permissions to create ECS, ALB, S3, DynamoDB, ECR, IAM, Route53, and ACM resources
2. Terraform >= 1.5
3. Docker (for building the runtime image)
4. A VPC with private subnets (for Fargate tasks). Public subnets are only needed for an internet-facing ALB. An **internal** ALB must use private subnets that have a return route to Client VPN / Transit Gateway (not `0.0.0.0/0` → IGW).
5. A Route53 hosted zone (optional — only needed if you want a custom domain)

## Quick Start

```bash
# 1. Initialize Terraform
cd deploy/terraform/examples/complete
terraform init

# 2. Create a tfvars file
cat > dev.tfvars <<EOF
environment        = "dev"
vpc_id             = "vpc-0123456789abcdef0"
private_subnet_ids = ["subnet-aaa", "subnet-bbb"]
public_subnet_ids  = ["subnet-ccc", "subnet-ddd"]
domain_name        = "veil.example.com"
hosted_zone_id     = "Z0123456789ABCDEFGHIJ"
EOF

# 3. Plan and apply
terraform plan -var-file="dev.tfvars"
terraform apply -var-file="dev.tfvars"

# 4. Build and push the Docker image
ECR_URL=$(terraform output -raw ecr_repository_url)
aws ecr get-login-password --region us-west-2 | docker login --username AWS --password-stdin $ECR_URL

# Build from the repo root
cd ../../../../
docker build -t veil-runtime -f deploy/Dockerfile .
docker tag veil-runtime:latest $ECR_URL:latest
docker push $ECR_URL:latest

# 5. Force a deployment
aws ecs update-service \
  --cluster $(terraform output -raw ecs_cluster_name) \
  --service veil-runtime \
  --force-new-deployment
```

## Module Reference

### Required Variables

| Variable | Type | Description |
|----------|------|-------------|
| `environment` | string | Environment name (dev, staging, prod) |
| `vpc_id` | string | VPC to deploy into |
| `subnet_ids` | list(string) | Private subnets for Fargate tasks |

### Optional Variables

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `name_prefix` | string | `"veil"` | Prefix for all resource names |
| `public_subnet_ids` | list(string) | `[]` | Public subnets for ALB |
| `domain_name` | string | `""` | Domain for HTTPS (e.g., `veil.example.com`) |
| `hosted_zone_id` | string | `""` | Route53 zone for DNS |
| `create_certificate` | bool | `true` | Create ACM certificate |
| `certificate_arn` | string | `""` | Existing ACM cert ARN |
| `create_s3_bucket` | bool | `true` | Create S3 bucket |
| `s3_bucket_name` | string | `""` | Existing bucket name |
| `create_ddb_table` | bool | `true` | Create DynamoDB table |
| `ddb_table_name` | string | `""` | Existing table name |
| `create_ecr` | bool | `true` | Create ECR repository |
| `ecr_repository_url` | string | `""` | Existing ECR URL |
| `create_cluster` | bool | `true` | Create ECS cluster |
| `cluster_arn` | string | `""` | Existing cluster ARN |
| `create_alb` | bool | `true` | Create ALB |
| `alb_internal` | bool | `false` | Make ALB internal |
| `task_cpu` | number | `1024` | Fargate task CPU units |
| `task_memory` | number | `2048` | Fargate task memory (MiB) |
| `desired_count` | number | `1` | Number of tasks |
| `image_tag` | string | `"latest"` | Docker image tag |
| `enable_autoscaling` | bool | `false` | Enable auto-scaling |
| `log_retention_days` | number | `14` | CloudWatch log retention |
| `alb_ingress_cidrs` | list(string) | `["0.0.0.0/0"]` | CIDR blocks for ALB access. For AWS Client VPN, include the VPN SNAT CIDRs (networking VPC, e.g. `10.7.0.0/16`) **and** the client CIDR (`172.16.0.0/12`) |
| `extra_environment` | map(string) | `{}` | Additional container env vars |

### Outputs

| Output | Description |
|--------|-------------|
| `service_url` | Full HTTPS URL for the service |
| `alb_dns_name` | ALB DNS name |
| `ecr_repository_url` | ECR push URL |
| `s3_bucket_name` | S3 bucket name |
| `ddb_table_name` | DynamoDB table name |
| `ecs_cluster_name` | ECS cluster name |
| `ecs_service_name` | ECS service name |
| `task_role_arn` | Task role ARN (for additional policies) |

## Resource Reuse Pattern

Every resource uses a `create_*` boolean. Set it to `false` and provide the existing resource identifier:

```hcl
module "veil" {
  source = "./modules/veil-runtime"

  # Use existing S3 bucket
  create_s3_bucket = false
  s3_bucket_name   = "my-existing-bucket"

  # Use existing DynamoDB table
  create_ddb_table = false
  ddb_table_name   = "my-existing-table"

  # Use existing ECS cluster
  create_cluster = false
  cluster_arn    = "arn:aws:ecs:us-west-2:123456789:cluster/my-cluster"

  # Use existing ECR repository
  create_ecr          = false
  ecr_repository_url  = "123456789.dkr.ecr.us-west-2.amazonaws.com/my-repo"

  # Use existing ACM certificate
  create_certificate = false
  certificate_arn    = "arn:aws:acm:us-west-2:123456789:certificate/abc-123"

  # ... other required vars
}
```

## Task Sizing

The VEIL runtime compiles generated code (Rust via `cargo check`, TypeScript via `tsc`). Recommended minimums:

| Environment | CPU | Memory | Notes |
|-------------|-----|--------|-------|
| Dev | 1024 (1 vCPU) | 2048 MiB | Single developer, occasional builds |
| Staging | 2048 (2 vCPU) | 4096 MiB | CI integration, parallel builds |
| Production | 2048+ (2+ vCPU) | 4096+ MiB | Multiple concurrent users |

## Dockerfile

The runtime image (`deploy/Dockerfile`) includes:
- **veil-runtime** — the ProductHost server binary
- **veil** CLI — for check/gen operations
- **Rust toolchain** — `cargo check` on generated Rust projects
- **Node.js 20** — `tsc` / `svelte-check` for TypeScript/Svelte
- **git** — version control for project repositories
- System layers and stubs at `/opt/veil/layers/` and `/opt/veil/stubs/`
- Built UI SPA at `/opt/veil/ui/`

Build from the repository root:

```bash
docker build -t veil-runtime -f deploy/Dockerfile .
```

## Environment Variables

The container receives these environment variables automatically:

| Variable | Value | Description |
|----------|-------|-------------|
| `VEIL_PORT` | `8080` | HTTP listen port |
| `VEIL_S3_BUCKET` | (from module) | Project/git storage bucket |
| `BUCKET` | (from module) | Alias for S3 bucket |
| `VEIL_DDB_TABLE` | (from module) | Metadata table |
| `AWS_REGION` | (from provider) | AWS region |
| `RUST_LOG` | `info,veil=debug` | Log level |
| `VEIL_LAYERS_DIR` | `/opt/veil/layers` | System layers path |
| `VEIL_UI_DIR` | `/opt/veil/ui` | Built UI path |

Add custom env vars via `extra_environment`:

```hcl
extra_environment = {
  VEIL_FEATURE_FLAG = "true"
  CUSTOM_SETTING    = "value"
}
```
