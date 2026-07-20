terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 5.0"
    }
  }
}

data "aws_region" "current" {}
data "aws_caller_identity" "current" {}

locals {
  prefix      = "${var.name_prefix}-${var.environment}"
  account_id  = data.aws_caller_identity.current.account_id
  region      = data.aws_region.current.name
  cluster_arn = var.create_cluster ? aws_ecs_cluster.this[0].arn : var.cluster_arn

  bucket_name = var.projects_bucket_name != "" ? var.projects_bucket_name : "${local.prefix}-projects"
  table_name  = var.meta_table_name != "" ? var.meta_table_name : "${local.prefix}-meta"

  tags = merge(var.tags, {
    Project     = "veil"
    Environment = var.environment
    ManagedBy   = "terraform"
  })
}

# ═══════════════════════════════════════════════════════════════════════════════
# ECR Repository
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_ecr_repository" "runtime" {
  name                 = "${local.prefix}-runtime"
  image_tag_mutability = "MUTABLE"
  force_delete         = var.environment == "dev"

  image_scanning_configuration {
    scan_on_push = true
  }

  tags = local.tags
}

resource "aws_ecr_lifecycle_policy" "runtime" {
  repository = aws_ecr_repository.runtime.name
  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "Keep last 10 images"
      selection = {
        tagStatus   = "any"
        countType   = "imageCountMoreThan"
        countNumber = 10
      }
      action = { type = "expire" }
    }]
  })
}

# ═══════════════════════════════════════════════════════════════════════════════
# ECS Cluster + EC2 Capacity
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

# ECS-optimized AMI
data "aws_ssm_parameter" "ecs_ami" {
  name = "/aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id"
}

# IAM role for EC2 instances
resource "aws_iam_role" "ecs_instance" {
  name = "${local.prefix}-ecs-instance"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = { Service = "ec2.amazonaws.com" }
    }]
  })

  tags = local.tags
}

resource "aws_iam_role_policy_attachment" "ecs_instance" {
  role       = aws_iam_role.ecs_instance.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonEC2ContainerServiceforEC2Role"
}

resource "aws_iam_role_policy_attachment" "ecs_instance_ssm" {
  role       = aws_iam_role.ecs_instance.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

resource "aws_iam_instance_profile" "ecs_instance" {
  name = "${local.prefix}-ecs-instance"
  role = aws_iam_role.ecs_instance.name
}

# Security group for ECS instances
resource "aws_security_group" "ecs_instances" {
  name_prefix = "${local.prefix}-ecs-"
  vpc_id      = var.vpc_id
  description = "ECS container instances for veil-runtime"

  ingress {
    description = "Allow traffic from ALB"
    from_port   = 0
    to_port     = 65535
    protocol    = "tcp"
    self        = true
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = local.tags
}

# Launch template
resource "aws_launch_template" "ecs" {
  name_prefix   = "${local.prefix}-ecs-"
  image_id      = data.aws_ssm_parameter.ecs_ami.value
  instance_type = var.instance_type

  iam_instance_profile {
    arn = aws_iam_instance_profile.ecs_instance.arn
  }

  vpc_security_group_ids = concat(
    [aws_security_group.ecs_instances.id],
    var.security_group_ids
  )

  key_name = var.key_name != "" ? var.key_name : null

  user_data = base64encode(<<-EOF
    #!/bin/bash
    echo "ECS_CLUSTER=${var.create_cluster ? aws_ecs_cluster.this[0].name : split("/", var.cluster_arn)[1]}" >> /etc/ecs/ecs.config
    echo "ECS_ENABLE_CONTAINER_METADATA=true" >> /etc/ecs/ecs.config
  EOF
  )

  tag_specifications {
    resource_type = "instance"
    tags = merge(local.tags, {
      Name = "${local.prefix}-ecs-instance"
    })
  }
}

# Auto Scaling Group
resource "aws_autoscaling_group" "ecs" {
  name_prefix         = "${local.prefix}-ecs-"
  min_size            = var.min_instances
  max_size            = var.max_instances
  desired_capacity    = var.min_instances
  vpc_zone_identifier = var.subnet_ids

  launch_template {
    id      = aws_launch_template.ecs.id
    version = "$Latest"
  }

  tag {
    key                 = "AmazonECSManaged"
    value               = "true"
    propagate_at_launch = true
  }

  lifecycle {
    create_before_destroy = true
  }
}

# Capacity provider
resource "aws_ecs_capacity_provider" "ec2" {
  name = "${local.prefix}-ec2"

  auto_scaling_group_provider {
    auto_scaling_group_arn         = aws_autoscaling_group.ecs.arn
    managed_termination_protection = "DISABLED"

    managed_scaling {
      status          = "ENABLED"
      target_capacity = 100
    }
  }

  tags = local.tags
}

resource "aws_ecs_cluster_capacity_providers" "this" {
  count        = var.create_cluster ? 1 : 0
  cluster_name = aws_ecs_cluster.this[0].name

  capacity_providers = [aws_ecs_capacity_provider.ec2.name]

  default_capacity_provider_strategy {
    capacity_provider = aws_ecs_capacity_provider.ec2.name
    weight            = 1
  }
}

# ═══════════════════════════════════════════════════════════════════════════════
# Storage — S3 + DynamoDB
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_s3_bucket" "projects" {
  count         = var.create_storage ? 1 : 0
  bucket        = local.bucket_name
  force_destroy = var.environment == "dev"
  tags          = local.tags
}

resource "aws_s3_bucket_versioning" "projects" {
  count  = var.create_storage ? 1 : 0
  bucket = aws_s3_bucket.projects[0].id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_dynamodb_table" "meta" {
  count        = var.create_storage ? 1 : 0
  name         = local.table_name
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

  tags = local.tags
}

# ═══════════════════════════════════════════════════════════════════════════════
# Task Execution & Task Role
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_iam_role" "task_execution" {
  name = "${local.prefix}-task-execution"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })

  tags = local.tags
}

resource "aws_iam_role_policy_attachment" "task_execution" {
  role       = aws_iam_role.task_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role" "task" {
  name = "${local.prefix}-task"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action = "sts:AssumeRole"
      Effect = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })

  tags = local.tags
}

# Task role policy — access to S3 and DynamoDB
resource "aws_iam_role_policy" "task" {
  name = "${local.prefix}-task-policy"
  role = aws_iam_role.task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:ListBucket",
        ]
        Resource = var.create_storage ? [
          aws_s3_bucket.projects[0].arn,
          "${aws_s3_bucket.projects[0].arn}/*",
        ] : ["*"]
      },
      {
        Effect = "Allow"
        Action = [
          "dynamodb:GetItem",
          "dynamodb:PutItem",
          "dynamodb:DeleteItem",
          "dynamodb:Query",
          "dynamodb:Scan",
          "dynamodb:UpdateItem",
        ]
        Resource = var.create_storage ? [
          aws_dynamodb_table.meta[0].arn,
          "${aws_dynamodb_table.meta[0].arn}/index/*",
        ] : ["*"]
      },
    ]
  })
}

# ═══════════════════════════════════════════════════════════════════════════════
# ALB (optional)
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_security_group" "alb" {
  count       = var.create_alb ? 1 : 0
  name_prefix = "${local.prefix}-alb-"
  vpc_id      = var.vpc_id
  description = "ALB for veil-runtime"

  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = local.tags
}

resource "aws_lb" "this" {
  count              = var.create_alb ? 1 : 0
  name               = "${local.prefix}-alb"
  internal           = false
  load_balancer_type = "application"
  security_groups    = [aws_security_group.alb[0].id]
  subnets            = var.public_subnet_ids

  tags = local.tags
}

resource "aws_lb_target_group" "runtime" {
  count       = var.create_alb ? 1 : 0
  name        = "${local.prefix}-runtime"
  port        = var.runtime_port
  protocol    = "HTTP"
  vpc_id      = var.vpc_id
  target_type = "instance"

  health_check {
    path                = "/health"
    interval            = 30
    timeout             = 5
    healthy_threshold   = 2
    unhealthy_threshold = 3
  }

  tags = local.tags
}

resource "aws_lb_listener" "http" {
  count             = var.create_alb && var.certificate_arn == "" ? 1 : 0
  load_balancer_arn = aws_lb.this[0].arn
  port              = 80
  protocol          = "HTTP"

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.runtime[0].arn
  }
}

resource "aws_lb_listener" "https" {
  count             = var.create_alb && var.certificate_arn != "" ? 1 : 0
  load_balancer_arn = aws_lb.this[0].arn
  port              = 443
  protocol          = "HTTPS"
  certificate_arn   = var.certificate_arn

  default_action {
    type             = "forward"
    target_group_arn = aws_lb_target_group.runtime[0].arn
  }
}

# ═══════════════════════════════════════════════════════════════════════════════
# ECS Task Definition & Service
# ═══════════════════════════════════════════════════════════════════════════════

resource "aws_cloudwatch_log_group" "runtime" {
  name              = "/ecs/${local.prefix}-runtime"
  retention_in_days = 14
  tags              = local.tags
}

resource "aws_ecs_task_definition" "runtime" {
  family                   = "${local.prefix}-runtime"
  requires_compatibilities = ["EC2"]
  network_mode             = "bridge"
  execution_role_arn       = aws_iam_role.task_execution.arn
  task_role_arn            = aws_iam_role.task.arn

  container_definitions = jsonencode([{
    name  = "veil-runtime"
    image = "${aws_ecr_repository.runtime.repository_url}:${var.image_tag}"

    cpu       = var.runtime_cpu
    memory    = var.runtime_memory
    essential = true

    portMappings = [{
      containerPort = var.runtime_port
      hostPort      = var.runtime_port
      protocol      = "tcp"
    }]

    environment = [
      { name = "VEIL_PORT", value = tostring(var.runtime_port) },
      { name = "VEIL_STORAGE", value = "s3" },
      { name = "VEIL_S3_BUCKET", value = local.bucket_name },
      { name = "VEIL_META", value = "ddb" },
      { name = "VEIL_DDB_TABLE", value = local.table_name },
      { name = "AWS_REGION", value = local.region },
      { name = "RUST_LOG", value = "info,veil=debug" },
    ]

    logConfiguration = {
      logDriver = "awslogs"
      options = {
        "awslogs-group"         = aws_cloudwatch_log_group.runtime.name
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

resource "aws_ecs_service" "runtime" {
  name            = "${local.prefix}-runtime"
  cluster         = local.cluster_arn
  task_definition = aws_ecs_task_definition.runtime.arn
  desired_count   = var.desired_count

  capacity_provider_strategy {
    capacity_provider = aws_ecs_capacity_provider.ec2.name
    weight            = 1
  }

  dynamic "load_balancer" {
    for_each = var.create_alb ? [1] : []
    content {
      target_group_arn = aws_lb_target_group.runtime[0].arn
      container_name   = "veil-runtime"
      container_port   = var.runtime_port
    }
  }

  depends_on = [
    aws_iam_role_policy_attachment.task_execution,
  ]

  tags = local.tags
}
