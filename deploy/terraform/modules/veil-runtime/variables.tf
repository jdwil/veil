variable "environment" {
  description = "Environment name (dev, staging, prod)"
  type        = string
  default     = "dev"
}

variable "name_prefix" {
  description = "Prefix for all resource names"
  type        = string
  default     = "veil"
}

# ─── Networking (passed in from host infrastructure) ────────────────────────

variable "vpc_id" {
  description = "VPC to deploy into"
  type        = string
}

variable "subnet_ids" {
  description = "Private subnets for ECS tasks and EC2 instances"
  type        = list(string)
}

variable "public_subnet_ids" {
  description = "Public subnets for ALB (if create_alb = true)"
  type        = list(string)
  default     = []
}

variable "security_group_ids" {
  description = "Additional security groups to attach (e.g. VPN access)"
  type        = list(string)
  default     = []
}

# ─── Cluster ────────────────────────────────────────────────────────────────

variable "create_cluster" {
  description = "Whether to create a new ECS cluster or use an existing one"
  type        = bool
  default     = true
}

variable "cluster_arn" {
  description = "ARN of existing ECS cluster (when create_cluster = false)"
  type        = string
  default     = ""
}

variable "instance_type" {
  description = "EC2 instance type for ECS container instances"
  type        = string
  default     = "t3.medium"
}

variable "min_instances" {
  description = "Minimum number of EC2 instances in the ASG"
  type        = number
  default     = 1
}

variable "max_instances" {
  description = "Maximum number of EC2 instances in the ASG"
  type        = number
  default     = 2
}

variable "key_name" {
  description = "EC2 key pair name for SSH access (optional)"
  type        = string
  default     = ""
}

# ─── Service ────────────────────────────────────────────────────────────────

variable "runtime_cpu" {
  description = "CPU units for veil-runtime task (1024 = 1 vCPU)"
  type        = number
  default     = 512
}

variable "runtime_memory" {
  description = "Memory (MB) for veil-runtime task"
  type        = number
  default     = 1024
}

variable "runtime_port" {
  description = "Port the veil-runtime listens on"
  type        = number
  default     = 8080
}

variable "desired_count" {
  description = "Desired number of veil-runtime tasks"
  type        = number
  default     = 1
}

variable "image_tag" {
  description = "Docker image tag for veil-runtime"
  type        = string
  default     = "latest"
}

# ─── Storage ────────────────────────────────────────────────────────────────

variable "create_storage" {
  description = "Whether to create S3 bucket and DynamoDB table"
  type        = bool
  default     = true
}

variable "projects_bucket_name" {
  description = "S3 bucket name for VEIL projects/artifacts (when create_storage = true)"
  type        = string
  default     = ""
}

variable "meta_table_name" {
  description = "DynamoDB table name for metadata (when create_storage = true)"
  type        = string
  default     = ""
}

# ─── ALB ────────────────────────────────────────────────────────────────────

variable "create_alb" {
  description = "Whether to create an ALB for the runtime service"
  type        = bool
  default     = true
}

variable "alb_listener_arn" {
  description = "ARN of existing ALB listener (when create_alb = false)"
  type        = string
  default     = ""
}

variable "certificate_arn" {
  description = "ACM certificate ARN for HTTPS (optional, HTTP-only if empty)"
  type        = string
  default     = ""
}

variable "tags" {
  description = "Tags to apply to all resources"
  type        = map(string)
  default     = {}
}
