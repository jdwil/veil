output "cluster_arn" {
  description = "ECS cluster ARN"
  value       = local.cluster_arn
}

output "cluster_name" {
  description = "ECS cluster name"
  value       = var.create_cluster ? aws_ecs_cluster.this[0].name : split("/", var.cluster_arn)[1]
}

output "ecr_repository_url" {
  description = "ECR repository URL for pushing runtime images"
  value       = aws_ecr_repository.runtime.repository_url
}

output "service_name" {
  description = "ECS service name"
  value       = aws_ecs_service.runtime.name
}

output "alb_dns_name" {
  description = "ALB DNS name (if created)"
  value       = var.create_alb ? aws_lb.this[0].dns_name : ""
}

output "alb_url" {
  description = "Full ALB URL"
  value       = var.create_alb ? "http://${aws_lb.this[0].dns_name}" : ""
}

output "projects_bucket" {
  description = "S3 bucket name for projects/artifacts"
  value       = var.create_storage ? aws_s3_bucket.projects[0].id : ""
}

output "meta_table" {
  description = "DynamoDB table name for metadata"
  value       = var.create_storage ? aws_dynamodb_table.meta[0].name : ""
}

output "task_role_arn" {
  description = "Task IAM role ARN (for additional policy attachments)"
  value       = aws_iam_role.task.arn
}

output "task_execution_role_arn" {
  description = "Task execution IAM role ARN"
  value       = aws_iam_role.task_execution.arn
}

output "security_group_id" {
  description = "Security group ID for ECS instances"
  value       = aws_security_group.ecs_instances.id
}
