# ─── VEIL Frontend — S3 + CloudFront SPA Module ──────────────────────────────
#
# Generic Terraform module for deploying a VEIL-generated SvelteKit SPA.
# Provisions S3 bucket, CloudFront distribution, ACM certificate (us-east-1),
# Origin Access Identity, and Route53 DNS record.
#
# Resource naming: veil-{slug}-spa (NO environment suffix).
# Environments are segregated by AWS account, not by name.
#
# Usage:
#   module "my_frontend" {
#     source         = "github.com/unsung-operators/veil//deploy/terraform/modules/veil-frontend"
#     slug           = "dlx-ai"
#     domain         = "ai.dev.dashlx.com"
#     hosted_zone_id = data.aws_route53_zone.dev.zone_id
#     environment    = "dev"
#   }

terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source                = "hashicorp/aws"
      version               = ">= 5.0"
      configuration_aliases = [aws.us_east_1]
    }
  }
}

data "aws_caller_identity" "current" {}

locals {
  prefix      = "veil-${var.slug}-spa"
  bucket_name = "veil-${var.slug}-spa-${data.aws_caller_identity.current.account_id}"

  # Cache TTLs: short in dev, longer in prod
  default_ttl = var.environment == "prod" ? 86400 : 3600
  max_ttl     = var.environment == "prod" ? 604800 : 86400

  tags = merge(var.tags, {
    Service   = var.slug
    ManagedBy = "terraform"
    Module    = "veil-frontend"
  })
}

# ─── S3 Bucket ────────────────────────────────────────────────────────────────

resource "aws_s3_bucket" "spa" {
  bucket = local.bucket_name
  tags   = local.tags
}

resource "aws_s3_bucket_versioning" "spa" {
  bucket = aws_s3_bucket.spa.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_public_access_block" "spa" {
  bucket = aws_s3_bucket.spa.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# ─── CloudFront Origin Access Identity ────────────────────────────────────────

resource "aws_cloudfront_origin_access_identity" "spa" {
  comment = "OAI for ${local.prefix}"
}

# S3 bucket policy: allow CloudFront OAI read access only
resource "aws_s3_bucket_policy" "spa" {
  bucket = aws_s3_bucket.spa.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "AllowCloudFrontOAI"
      Effect    = "Allow"
      Principal = { AWS = aws_cloudfront_origin_access_identity.spa.iam_arn }
      Action    = "s3:GetObject"
      Resource  = "${aws_s3_bucket.spa.arn}/*"
    }]
  })

  depends_on = [aws_s3_bucket_public_access_block.spa]
}

# ─── ACM Certificate (us-east-1 — CloudFront requirement) ────────────────────

resource "aws_acm_certificate" "spa" {
  provider          = aws.us_east_1
  domain_name       = var.domain
  validation_method = "DNS"
  tags              = local.tags

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_route53_record" "cert_validation" {
  for_each = {
    for dvo in aws_acm_certificate.spa.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  }

  zone_id = var.hosted_zone_id
  name    = each.value.name
  type    = each.value.type
  ttl     = 60
  records = [each.value.record]

  allow_overwrite = true
}

resource "aws_acm_certificate_validation" "spa" {
  provider                = aws.us_east_1
  certificate_arn         = aws_acm_certificate.spa.arn
  validation_record_fqdns = [for record in aws_route53_record.cert_validation : record.fqdn]
}

# ─── CloudFront Distribution ─────────────────────────────────────────────────

resource "aws_cloudfront_distribution" "spa" {
  enabled             = true
  default_root_object = "index.html"
  price_class         = "PriceClass_100"
  aliases             = [var.domain]
  comment             = "${local.prefix} SPA distribution"

  origin {
    domain_name = aws_s3_bucket.spa.bucket_regional_domain_name
    origin_id   = "S3-${aws_s3_bucket.spa.id}"

    s3_origin_config {
      origin_access_identity = aws_cloudfront_origin_access_identity.spa.cloudfront_access_identity_path
    }
  }

  default_cache_behavior {
    allowed_methods        = ["GET", "HEAD", "OPTIONS"]
    cached_methods         = ["GET", "HEAD"]
    target_origin_id       = "S3-${aws_s3_bucket.spa.id}"
    viewer_protocol_policy = "redirect-to-https"
    compress               = true

    forwarded_values {
      query_string = false
      cookies {
        forward = "none"
      }
    }

    min_ttl     = 0
    default_ttl = local.default_ttl
    max_ttl     = local.max_ttl
  }

  # SPA client-side routing: 404 → index.html
  custom_error_response {
    error_code         = 403
    response_code      = 200
    response_page_path = "/index.html"
  }

  custom_error_response {
    error_code         = 404
    response_code      = 200
    response_page_path = "/index.html"
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.spa.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }

  tags = local.tags
}

# ─── Route53 DNS Record ──────────────────────────────────────────────────────

resource "aws_route53_record" "spa" {
  zone_id = var.hosted_zone_id
  name    = var.domain
  type    = "A"

  alias {
    name                   = aws_cloudfront_distribution.spa.domain_name
    zone_id                = aws_cloudfront_distribution.spa.hosted_zone_id
    evaluate_target_health = false
  }
}
