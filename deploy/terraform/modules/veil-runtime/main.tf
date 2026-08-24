# ═══════════════════════════════════════════════════════════════════════════════
# VEIL Runtime — Production Terraform Module
#
# Deploys the VEIL ProductHost on AWS Fargate with:
#   • ECS Cluster + Fargate Service
#   • ALB with HTTPS (ACM + Route53)
#   • S3 bucket for project/git storage
#   • DynamoDB table for metadata
#   • ECR repository for container images
#   • IAM roles (task execution + task)
#   • CloudWatch logging
#   • Optional auto-scaling
#
# Every resource can be created OR reused from existing infrastructure.
# ═══════════════════════════════════════════════════════════════════════════════

terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 5.0"
    }
  }
}

# ─── Data Sources ────────────────────────────────────────────────────────────

data "aws_region" "current" {}
data "aws_caller_identity" "current" {}

locals {
  prefix     = "${var.name_prefix}-${var.environment}"
  region     = data.aws_region.current.name
  account_id = data.aws_caller_identity.current.account_id

  cluster_arn  = var.create_cluster ? aws_ecs_cluster.this[0].arn : var.cluster_arn
  cluster_name = var.create_cluster ? aws_ecs_cluster.this[0].name : element(split("/", var.cluster_arn), length(split("/", var.cluster_arn)) - 1)

  bucket_name = var.create_s3_bucket ? aws_s3_bucket.this[0].id : var.s3_bucket_name
  bucket_arn  = var.create_s3_bucket ? aws_s3_bucket.this[0].arn : "arn:aws:s3:::${var.s3_bucket_name}"
  table_name  = var.create_ddb_table ? aws_dynamodb_table.this[0].name : var.ddb_table_name
  table_arn   = var.create_ddb_table ? aws_dynamodb_table.this[0].arn : "arn:aws:dynamodb:${local.region}:${local.account_id}:table/${var.ddb_table_name}"
  ecr_url     = var.create_ecr ? aws_ecr_repository.this[0].repository_url : var.ecr_repository_url

  # Internet-facing ALBs go in public subnets. Internal ALBs MUST use private
  # subnet_ids: public RTs typically have 0.0.0.0/0 → IGW and no TGW/VPN return
  # path, so Client VPN traffic is blackholed on the way back.
  alb_subnets = var.alb_internal ? var.subnet_ids : (
    length(var.public_subnet_ids) > 0 ? var.public_subnet_ids : var.subnet_ids
  )

  # TLS/DNS
  has_domain      = var.domain_name != ""
  certificate_arn = var.create_certificate && local.has_domain ? aws_acm_certificate.this[0].arn : var.certificate_arn
  has_tls         = var.create_certificate || var.certificate_arn != ""

  tags = merge(var.tags, {
    Project     = "veil"
    Environment = var.environment
    ManagedBy   = "terraform"
  })
}

# ═══════════════════════════════════════════════════════════════════════════════
# ECR Repository
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_ecr_repository" "this" {
  count                = var.create_ecr ? 1 : 0
  name                 = "${local.prefix}-runtime"
  image_tag_mutability = "MUTABLE"
  force_delete         = var.environment != "prod"

  image_scanning_configuration {
    scan_on_push = true
  }

  tags = local.tags
}

resource "aws_ecr_lifecycle_policy" "this" {
  count      = var.create_ecr ? 1 : 0
  repository = aws_ecr_repository.this[0].name

  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "Keep last ${var.ecr_lifecycle_max_images} images"
      selection = {
        tagStatus   = "any"
        countType   = "imageCountMoreThan"
        countNumber = var.ecr_lifecycle_max_images
      }
      action = { type = "expire" }
    }]
  })
}

# ═══════════════════════════════════════════════════════════════════════════════
# ECS Cluster
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_ecs_cluster" "this" {
  count = var.create_cluster ? 1 : 0
  name  = "${local.prefix}-cluster"

  setting {
    name  = "containerInsights"
    value = "enabled"
  }

  tags = local.tags
}

# ═══════════════════════════════════════════════════════════════════════════════
# Storage — S3
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_s3_bucket" "this" {
  count         = var.create_s3_bucket ? 1 : 0
  bucket        = var.s3_bucket_name != "" ? var.s3_bucket_name : "${local.prefix}-projects"
  force_destroy = var.environment != "prod"
  tags          = local.tags
}

resource "aws_s3_bucket_versioning" "this" {
  count  = var.create_s3_bucket ? 1 : 0
  bucket = aws_s3_bucket.this[0].id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "this" {
  count  = var.create_s3_bucket ? 1 : 0
  bucket = aws_s3_bucket.this[0].id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "this" {
  count  = var.create_s3_bucket ? 1 : 0
  bucket = aws_s3_bucket.this[0].id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# ═══════════════════════════════════════════════════════════════════════════════
# Storage — DynamoDB
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_dynamodb_table" "this" {
  count        = var.create_ddb_table ? 1 : 0
  name         = var.ddb_table_name != "" ? var.ddb_table_name : "${local.prefix}-meta"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "pk"
  range_key    = "sk"

  attribute {
    name = "pk"
    type = "S"
  }

  attribute {
    name = "sk"
    type = "S"
  }

  point_in_time_recovery {
    enabled = var.environment == "prod"
  }

  tags = local.tags
}

# ═══════════════════════════════════════════════════════════════════════════════
# IAM — Task Execution Role (ECR pull + CloudWatch logs)
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_iam_role" "task_execution" {
  name = "${local.prefix}-task-exec"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })

  tags = local.tags
}

resource "aws_iam_role_policy_attachment" "task_execution" {
  role       = aws_iam_role.task_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

# ═══════════════════════════════════════════════════════════════════════════════
# IAM — Task Role (S3, DDB, and runtime AWS services)
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_iam_role" "task" {
  name = "${local.prefix}-task"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })

  tags = local.tags
}

resource "aws_iam_role_policy" "task_s3" {
  name = "${local.prefix}-task-s3"
  role = aws_iam_role.task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "s3:GetObject",
        "s3:PutObject",
        "s3:DeleteObject",
        "s3:ListBucket",
        "s3:GetBucketLocation",
      ]
      Resource = [
        local.bucket_arn,
        "${local.bucket_arn}/*",
      ]
    }]
  })
}

resource "aws_iam_role_policy" "task_ddb" {
  name = "${local.prefix}-task-ddb"
  role = aws_iam_role.task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "dynamodb:GetItem",
        "dynamodb:PutItem",
        "dynamodb:DeleteItem",
        "dynamodb:Query",
        "dynamodb:Scan",
        "dynamodb:UpdateItem",
        "dynamodb:BatchGetItem",
        "dynamodb:BatchWriteItem",
      ]
      Resource = [
        local.table_arn,
        "${local.table_arn}/index/*",
      ]
    }]
  })
}

# ═══════════════════════════════════════════════════════════════════════════════
# CloudWatch Logs
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_cloudwatch_log_group" "this" {
  name              = "/ecs/${local.prefix}-runtime"
  retention_in_days = var.log_retention_days
  tags              = local.tags
}

# ═══════════════════════════════════════════════════════════════════════════════
# Security Groups
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_security_group" "alb" {
  count       = var.create_alb ? 1 : 0
  name_prefix = "${local.prefix}-alb-"
  vpc_id      = var.vpc_id
  description = "ALB for VEIL runtime"

  ingress {
    description = "HTTPS"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = var.alb_ingress_cidrs
  }

  ingress {
    description = "HTTP (redirect to HTTPS)"
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = var.alb_ingress_cidrs
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  lifecycle {
    create_before_destroy = true
  }

  tags = local.tags
}

resource "aws_security_group" "ecs_tasks" {
  name_prefix = "${local.prefix}-ecs-"
  vpc_id      = var.vpc_id
  description = "ECS Fargate tasks for VEIL runtime"

  ingress {
    description     = "Allow traffic from ALB"
    from_port       = var.runtime_port
    to_port         = var.runtime_port
    protocol        = "tcp"
    security_groups = var.create_alb ? [aws_security_group.alb[0].id] : []
    cidr_blocks     = var.create_alb ? [] : ["0.0.0.0/0"]
  }

  egress {
    description = "All outbound (pull images, access AWS APIs, npm/cargo registries)"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  lifecycle {
    create_before_destroy = true
  }

  tags = local.tags
}

# ═══════════════════════════════════════════════════════════════════════════════
# ALB
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_lb" "this" {
  count              = var.create_alb ? 1 : 0
  name               = "${local.prefix}-alb"
  internal           = var.alb_internal
  load_balancer_type = "application"
  security_groups    = [aws_security_group.alb[0].id]
  subnets            = local.alb_subnets

  tags = local.tags
}

resource "aws_lb_target_group" "this" {
  count       = var.create_alb ? 1 : 0
  name        = "${local.prefix}-tg"
  port        = var.runtime_port
  protocol    = "HTTP"
  vpc_id      = var.vpc_id
  target_type = "ip"

  health_check {
    path                = "/health"
    interval            = 30
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
    matcher             = "200"
  }

  lifecycle {
    create_before_destroy = true
  }

  tags = local.tags
}

# HTTPS listener (when TLS is configured)
resource "aws_lb_listener" "https" {
  count             = var.create_alb && local.has_tls ? 1 : 0
  load_balancer_arn = aws_lb.this[0].arn
  port              = 443
  protocol          = "HTTPS"
  ssl_policy        = "ELBSecurityPolicy-TLS13-1-2-2021-06"
  certificate_arn   = local.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.this[0].arn
  }
}

# HTTP → HTTPS redirect (when TLS is configured)
resource "aws_lb_listener" "http_redirect" {
  count             = var.create_alb && local.has_tls ? 1 : 0
  load_balancer_arn = aws_lb.this[0].arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type = "redirect"
    redirect {
      port        = "443"
      protocol    = "HTTPS"
      status_code = "HTTP_301"
    }
  }
}

# HTTP-only listener (no TLS)
resource "aws_lb_listener" "http" {
  count             = var.create_alb && !local.has_tls ? 1 : 0
  load_balancer_arn = aws_lb.this[0].arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.this[0].arn
  }
}

# ═══════════════════════════════════════════════════════════════════════════════
# ACM Certificate + DNS Validation
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_acm_certificate" "this" {
  count             = var.create_certificate && local.has_domain ? 1 : 0
  domain_name       = var.domain_name
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }

  tags = local.tags
}

resource "aws_route53_record" "cert_validation" {
  for_each = var.create_certificate && local.has_domain ? {
    for dvo in aws_acm_certificate.this[0].domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  } : {}

  allow_overwrite = true
  name            = each.value.name
  records         = [each.value.record]
  ttl             = 60
  type            = each.value.type
  zone_id         = var.hosted_zone_id
}

resource "aws_acm_certificate_validation" "this" {
  count                   = var.create_certificate && local.has_domain ? 1 : 0
  certificate_arn         = aws_acm_certificate.this[0].arn
  validation_record_fqdns = [for record in aws_route53_record.cert_validation : record.fqdn]
}

# ═══════════════════════════════════════════════════════════════════════════════
# Route53 DNS Record (A alias → ALB)
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_route53_record" "this" {
  count   = local.has_domain && var.create_alb ? 1 : 0
  zone_id = var.hosted_zone_id
  name    = var.domain_name
  type    = "A"

  alias {
    name                   = aws_lb.this[0].dns_name
    zone_id                = aws_lb.this[0].zone_id
    evaluate_target_health = true
  }
}

# ═══════════════════════════════════════════════════════════════════════════════
# ECS Task Definition
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_ecs_task_definition" "this" {
  family                   = "${local.prefix}-runtime"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.task_execution.arn
  task_role_arn            = aws_iam_role.task.arn

  container_definitions = jsonencode([{
    name      = "veil-runtime"
    image     = "${local.ecr_url}:${var.image_tag}"
    essential = true

    portMappings = [{
      containerPort = var.runtime_port
      protocol      = "tcp"
    }]

    environment = concat(
      [
        { name = "VEIL_PORT", value = tostring(var.runtime_port) },
        { name = "VEIL_S3_BUCKET", value = local.bucket_name },
        { name = "BUCKET", value = local.bucket_name },
        { name = "VEIL_DDB_TABLE", value = local.table_name },
        { name = "AWS_REGION", value = local.region },
        { name = "RUST_LOG", value = "info,veil=debug" },
        { name = "VEIL_LAYERS_DIR", value = "/opt/veil/layers" },
        { name = "VEIL_UI_DIR", value = "/opt/veil/ui" },
      ],
      [for k, v in var.extra_environment : { name = k, value = v }]
    )

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.this.name
        "awslogs-region"        = local.region
        "awslogs-stream-prefix" = "runtime"
      }
    }

    healthCheck = {
      command     = ["CMD-SHELL", "curl -f http://localhost:${var.runtime_port}/health || exit 1"]
      interval    = 30
      timeout     = 5
      retries     = 3
      startPeriod = 60
    }
  }])

  tags = local.tags
}

# ═══════════════════════════════════════════════════════════════════════════════
# ECS Service
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_ecs_service" "this" {
  name            = "${local.prefix}-runtime"
  cluster         = local.cluster_arn
  task_definition = aws_ecs_task_definition.this.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"

  network_configuration {
    subnets          = var.subnet_ids
    security_groups  = [aws_security_group.ecs_tasks.id]
    assign_public_ip = var.assign_public_ip
  }

  dynamic "load_balancer" {
    for_each = var.create_alb ? [1] : []
    content {
      target_group_arn = aws_lb_target_group.this[0].arn
      container_name   = "veil-runtime"
      container_port   = var.runtime_port
    }
  }

  depends_on = [
    aws_iam_role_policy_attachment.task_execution,
    aws_lb_listener.https,
    aws_lb_listener.http,
  ]

  lifecycle {
    ignore_changes = [desired_count]
  }

  tags = local.tags
}

# ═══════════════════════════════════════════════════════════════════════════════
# Auto-scaling (optional)
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_appautoscaling_target" "this" {
  count              = var.enable_autoscaling ? 1 : 0
  max_capacity       = var.autoscaling_max_capacity
  min_capacity       = var.autoscaling_min_capacity
  resource_id        = "service/${local.cluster_name}/${aws_ecs_service.this.name}"
  scalable_dimension = "ecs:service:DesiredCount"
  service_namespace  = "ecs"
}

resource "aws_appautoscaling_policy" "cpu" {
  count              = var.enable_autoscaling ? 1 : 0
  name               = "${local.prefix}-cpu-scaling"
  policy_type        = "TargetTrackingScaling"
  resource_id        = aws_appautoscaling_target.this[0].resource_id
  scalable_dimension = aws_appautoscaling_target.this[0].scalable_dimension
  service_namespace  = aws_appautoscaling_target.this[0].service_namespace

  target_tracking_scaling_policy_configuration {
    predefined_metric_specification {
      predefined_metric_type = "ECSServiceAverageCPUUtilization"
    }
    target_value       = var.autoscaling_cpu_target
    scale_in_cooldown  = 300
    scale_out_cooldown = 60
  }
}
