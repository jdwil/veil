# ═══════════════════════════════════════════════════════════════════════════════
# VEIL Runtime — Terraform Module Variables
# ═══════════════════════════════════════════════════════════════════════════════

# ─── General ─────────────────────────────────────────────────────────────────

variable "environment" {
  description = "Environment name (dev, staging, prod)"
  type        = string
}

variable "name_prefix" {
  description = "Prefix for all resource names"
  type        = string
  default     = "veil"
}

variable "tags" {
  description = "Additional tags to apply to all resources"
  type        = map(string)
  default     = {}
}

# ─── Networking ──────────────────────────────────────────────────────────────

variable "vpc_id" {
  description = "VPC to deploy into"
  type        = string
}

variable "subnet_ids" {
  description = "Private subnets for ECS Fargate tasks"
  type        = list(string)
}

variable "public_subnet_ids" {
  description = "Public subnets for an internet-facing ALB. Ignored when alb_internal=true (subnet_ids are used instead so VPN/TGW return routing works)."
  type        = list(string)
  default     = []
}

variable "alb_ingress_cidrs" {
  description = "CIDR blocks allowed to access the ALB (default: open to internet)"
  type        = list(string)
  default     = ["0.0.0.0/0"]
}

# ─── ECS Cluster ─────────────────────────────────────────────────────────────

variable "create_cluster" {
  description = "Whether to create a new ECS cluster"
  type        = bool
  default     = true
}

variable "cluster_arn" {
  description = "ARN of existing ECS cluster (required if create_cluster = false)"
  type        = string
  default     = ""
}

# ─── ECS Service ─────────────────────────────────────────────────────────────

variable "task_cpu" {
  description = "CPU units for the Fargate task (256, 512, 1024, 2048, 4096)"
  type        = number
  default     = 1024
}

variable "task_memory" {
  description = "Memory (MiB) for the Fargate task"
  type        = number
  default     = 2048
}

variable "desired_count" {
  description = "Desired number of running tasks"
  type        = number
  default     = 1
}

variable "image_tag" {
  description = "Docker image tag for veil-runtime"
  type        = string
  default     = "latest"
}

variable "runtime_port" {
  description = "Port the veil-runtime container listens on"
  type        = number
  default     = 8080
}

variable "assign_public_ip" {
  description = "Whether to assign a public IP to Fargate tasks (required if using public subnets without NAT)"
  type        = bool
  default     = false
}

# ─── Auto-scaling ────────────────────────────────────────────────────────────

variable "enable_autoscaling" {
  description = "Whether to enable auto-scaling for the ECS service"
  type        = bool
  default     = false
}

variable "autoscaling_min_capacity" {
  description = "Minimum number of tasks when auto-scaling is enabled"
  type        = number
  default     = 1
}

variable "autoscaling_max_capacity" {
  description = "Maximum number of tasks when auto-scaling is enabled"
  type        = number
  default     = 4
}

variable "autoscaling_cpu_target" {
  description = "Target CPU utilization percentage for auto-scaling"
  type        = number
  default     = 70
}

# ─── ALB ─────────────────────────────────────────────────────────────────────

variable "create_alb" {
  description = "Whether to create a new ALB"
  type        = bool
  default     = true
}

variable "alb_arn" {
  description = "ARN of existing ALB (required if create_alb = false and you want load balancing)"
  type        = string
  default     = ""
}

variable "alb_internal" {
  description = "Whether the ALB is internal (not internet-facing)"
  type        = bool
  default     = false
}

# ─── DNS / TLS ───────────────────────────────────────────────────────────────

variable "domain_name" {
  description = "Domain name for the VEIL runtime (e.g., veil.example.com). Leave empty to skip DNS/TLS."
  type        = string
  default     = ""
}

variable "hosted_zone_id" {
  description = "Route53 hosted zone ID for DNS record creation (required if domain_name is set)"
  type        = string
  default     = ""
}

variable "create_certificate" {
  description = "Whether to create an ACM certificate (set false to use existing)"
  type        = bool
  default     = true
}

variable "certificate_arn" {
  description = "ARN of existing ACM certificate (required if create_certificate = false and domain_name is set)"
  type        = string
  default     = ""
}

# ─── Storage: S3 ─────────────────────────────────────────────────────────────

variable "create_s3_bucket" {
  description = "Whether to create a new S3 bucket for project storage"
  type        = bool
  default     = true
}

variable "s3_bucket_name" {
  description = "Name of S3 bucket (required if create_s3_bucket = false, otherwise auto-generated)"
  type        = string
  default     = ""
}

# ─── Storage: DynamoDB ───────────────────────────────────────────────────────

variable "create_ddb_table" {
  description = "Whether to create a new DynamoDB table for metadata"
  type        = bool
  default     = true
}

variable "ddb_table_name" {
  description = "Name of DynamoDB table (required if create_ddb_table = false, otherwise auto-generated)"
  type        = string
  default     = ""
}

# ─── Container Registry ─────────────────────────────────────────────────────

variable "create_ecr" {
  description = "Whether to create an ECR repository"
  type        = bool
  default     = true
}

variable "ecr_repository_url" {
  description = "URL of existing ECR repository (required if create_ecr = false)"
  type        = string
  default     = ""
}

variable "ecr_lifecycle_max_images" {
  description = "Maximum number of images to keep in ECR"
  type        = number
  default     = 20
}

# ─── Logging ─────────────────────────────────────────────────────────────────

variable "log_retention_days" {
  description = "CloudWatch log retention in days"
  type        = number
  default     = 14
}

# ─── Extra environment variables ─────────────────────────────────────────────

variable "extra_environment" {
  description = "Additional environment variables to pass to the container"
  type        = map(string)
  default     = {}
}
