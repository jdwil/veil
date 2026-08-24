# ═══════════════════════════════════════════════════════════════════════════════
# VEIL Runtime — Complete Example
#
# This example shows a production-ready deployment with:
#   • New ECS cluster and ECR repository
#   • ALB with HTTPS (ACM certificate + Route53 DNS)
#   • Reuse of existing S3 bucket and DynamoDB table
#   • Auto-scaling enabled
#
# Usage:
#   cd deploy/terraform/examples/complete
#   terraform init
#   terraform plan -var-file="dev.tfvars"
#   terraform apply -var-file="dev.tfvars"
# ═══════════════════════════════════════════════════════════════════════════════

terraform {
  required_version = ">= 1.5"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  # Configure your backend here:
  # backend "s3" {
  #   bucket         = "my-terraform-state"
  #   key            = "veil-runtime/terraform.tfstate"
  #   region         = "us-west-2"
  #   dynamodb_table = "terraform-state-lock"
  # }
}

provider "aws" {
  region = var.aws_region
}

# ─── Variables ───────────────────────────────────────────────────────────────

variable "aws_region" {
  type    = string
  default = "us-west-2"
}

variable "environment" {
  type    = string
  default = "dev"
}

variable "vpc_id" {
  type        = string
  description = "VPC ID to deploy into"
}

variable "private_subnet_ids" {
  type        = list(string)
  description = "Private subnets for Fargate tasks"
}

variable "public_subnet_ids" {
  type        = list(string)
  description = "Public subnets for the ALB"
}

variable "domain_name" {
  type        = string
  description = "Domain name (e.g., veil.example.com)"
}

variable "hosted_zone_id" {
  type        = string
  description = "Route53 hosted zone ID"
}

variable "image_tag" {
  type    = string
  default = "latest"
}

# ─── Module instantiation ────────────────────────────────────────────────────

module "veil" {
  source = "../../modules/veil-runtime"

  environment = var.environment
  vpc_id      = var.vpc_id
  subnet_ids  = var.private_subnet_ids

  # ALB in public subnets, internet-facing
  public_subnet_ids = var.public_subnet_ids
  alb_internal      = false

  # DNS + TLS (auto-creates ACM cert and validates via Route53)
  domain_name    = var.domain_name
  hosted_zone_id = var.hosted_zone_id

  # Container image
  image_tag = var.image_tag

  # Task sizing (the runtime compiles code, so give it resources)
  task_cpu    = 2048
  task_memory = 4096

  # Storage: create fresh bucket and table
  create_s3_bucket = true
  create_ddb_table = true

  # Auto-scaling for production
  enable_autoscaling       = var.environment == "prod"
  autoscaling_min_capacity = 2
  autoscaling_max_capacity = 6
  autoscaling_cpu_target   = 60

  # Logging
  log_retention_days = var.environment == "prod" ? 90 : 14

  tags = {
    Team = "platform"
  }
}

# ─── Outputs ─────────────────────────────────────────────────────────────────

output "service_url" {
  value = module.veil.service_url
}

output "ecr_repository_url" {
  value = module.veil.ecr_repository_url
}

output "s3_bucket_name" {
  value = module.veil.s3_bucket_name
}

output "ddb_table_name" {
  value = module.veil.ddb_table_name
}

output "ecs_cluster_name" {
  value = module.veil.ecs_cluster_name
}

output "deploy_command" {
  description = "Command to build and push the Docker image"
  value       = <<-EOT
    # Build and push:
    aws ecr get-login-password --region ${var.aws_region} | docker login --username AWS --password-stdin ${module.veil.ecr_repository_url}
    docker build -t veil-runtime -f deploy/Dockerfile .
    docker tag veil-runtime:latest ${module.veil.ecr_repository_url}:${var.image_tag}
    docker push ${module.veil.ecr_repository_url}:${var.image_tag}

    # Force new deployment:
    aws ecs update-service --cluster ${module.veil.ecs_cluster_name} --service ${module.veil.ecs_service_name} --force-new-deployment
  EOT
}
