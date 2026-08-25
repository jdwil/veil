# ─── VEIL Frontend — Outputs ──────────────────────────────────────────────────
#
# Captured by the VEIL deploy pipeline and stored in project metadata.
# Used by CI/CD for S3 sync and CloudFront invalidation.

output "bucket_name" {
  description = "S3 bucket name for uploading built SPA assets."
  value       = aws_s3_bucket.spa.id
}

output "cloudfront_distribution_id" {
  description = "CloudFront distribution ID (used for cache invalidation)."
  value       = aws_cloudfront_distribution.spa.id
}

output "cloudfront_domain_name" {
  description = "CloudFront distribution domain name."
  value       = aws_cloudfront_distribution.spa.domain_name
}

output "site_url" {
  description = "Full HTTPS URL for the deployed SPA."
  value       = "https://${var.domain}"
}
