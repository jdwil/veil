# VEIL Runtime — Terraform Module

Reusable Terraform module for deploying `veil-runtime` to AWS ECS (EC2-backed).

## Usage

### Standalone (in this repo)

```hcl
module "veil_runtime" {
  source = "./deploy/terraform/modules/veil-runtime"

  environment       = "dev"
  vpc_id            = "vpc-xxxxx"
  subnet_ids        = ["subnet-aaa", "subnet-bbb"]
  public_subnet_ids = ["subnet-ccc", "subnet-ddd"]
}
```

### From another repo (e.g. dlx-core)

```hcl
module "veil_runtime" {
  source = "git::ssh://git@github.com/jdwil/veil.git//deploy/terraform/modules/veil-runtime?ref=main"

  environment        = "dev"
  vpc_id             = module.networking.vpc_id
  subnet_ids         = module.networking.private_subnet_ids
  public_subnet_ids  = module.networking.public_subnet_ids
  security_group_ids = [module.networking.vpn_sg_id]
  
  instance_type = "t3.medium"
  image_tag     = "latest"
}
```

## What it creates

- **ECR Repository** — for pushing runtime Docker images
- **ECS Cluster** (optional) — EC2-backed with managed capacity provider
- **Auto Scaling Group** — ECS-optimized instances (default: 1x t3.medium)
- **ECS Service + Task Definition** — always-on veil-runtime daemon
- **ALB** (optional) — HTTP/HTTPS load balancer
- **S3 Bucket** — project sources and build artifacts
- **DynamoDB Table** — metadata store (pk/sk schema)
- **IAM Roles** — task execution, task (S3 + DDB access), instance profile
- **CloudWatch Log Group** — 14-day retention

## Inputs

| Variable | Description | Default |
|----------|-------------|---------|
| `vpc_id` | VPC to deploy into | required |
| `subnet_ids` | Private subnets for ECS | required |
| `public_subnet_ids` | Public subnets for ALB | `[]` |
| `security_group_ids` | Additional SGs (VPN, etc.) | `[]` |
| `instance_type` | EC2 instance type | `t3.medium` |
| `create_cluster` | Create new ECS cluster | `true` |
| `create_alb` | Create ALB | `true` |
| `create_storage` | Create S3 + DDB | `true` |
| `image_tag` | Docker image tag | `latest` |
| `environment` | Environment name | `dev` |

See `variables.tf` for the full list.

## Outputs

| Output | Description |
|--------|-------------|
| `ecr_repository_url` | Push images here |
| `alb_url` | Runtime API endpoint |
| `cluster_arn` | ECS cluster ARN |
| `projects_bucket` | S3 bucket name |
| `meta_table` | DynamoDB table name |

## Deploying an image

```bash
# Build
cd /path/to/veil
docker build -f deploy/Dockerfile -t veil-runtime .

# Push to ECR
aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin <account>.dkr.ecr.us-east-1.amazonaws.com
docker tag veil-runtime:latest <ecr_url>:latest
docker push <ecr_url>:latest

# Force new deployment
aws ecs update-service --cluster veil-dev-cluster --service veil-dev-runtime --force-new-deployment
```
