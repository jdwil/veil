# ═══════════════════════════════════════════════════════════════════════════════
# VEIL Runtime — Terraform Module Outputs
# ═══════════════════════════════════════════════════════════════════════════════

output "alb_dns_name" {
  description = "ALB DNS name"
  value       = var.create_alb ? aws_lb.this[0].dns_name : ""
}

output "service_url" {
  description = "Full URL for the VEIL runtime service"
  value       = local.has_domain ? "https://${var.domain_name}" : (var.create_alb ? "http://${aws_lb.this[0].dns_name}" : "")
}

output "ecr_repository_url" {
  description = "ECR repository URL for pushing runtime images"
  value       = local.ecr_url
}

output "s3_bucket_name" {
  description = "S3 bucket name for project storage"
  value       = local.bucket_name
}

output "ddb_table_name" {
  description = "DynamoDB table name for metadata"
  value       = local.table_name
}

output "ecs_cluster_name" {
  description = "ECS cluster name"
  value       = local.cluster_name
}

output "ecs_cluster_arn" {
  description = "ECS cluster ARN"
  value       = local.cluster_arn
}

output "ecs_service_name" {
  description = "ECS service name"
  value       = aws_ecs_service.this.name
}

output "task_role_arn" {
  description = "IAM role ARN for the ECS task (attach additional policies here)"
  value       = aws_iam_role.task.arn
}

output "task_execution_role_arn" {
  description = "IAM role ARN for ECS task execution"
  value       = aws_iam_role.task_execution.arn
}

output "ecs_security_group_id" {
  description = "Security group ID for ECS tasks"
  value       = aws_security_group.ecs_tasks.id
}

output "alb_security_group_id" {
  description = "Security group ID for the ALB (empty if ALB not created)"
  value       = var.create_alb ? aws_security_group.alb[0].id : ""
}

output "log_group_name" {
  description = "CloudWatch log group name"
  value       = aws_cloudwatch_log_group.this.name
}
