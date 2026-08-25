# ─── VEIL Frontend — Variables ────────────────────────────────────────────────
#
# Generic SPA deployment module.
# Requires a provider alias `aws.us_east_1` for ACM certificates.

# ─── Required ─────────────────────────────────────────────────────────────────

variable "slug" {
  description = "Project slug — used for bucket and resource naming (e.g. 'dlx-ai')."
  type        = string
}

variable "domain" {
  description = "Domain name for the SPA (e.g. 'ai.dev.dashlx.com')."
  type        = string
}

variable "hosted_zone_id" {
  description = "Route53 hosted zone ID for the domain."
  type        = string
}

# ─── Optional ─────────────────────────────────────────────────────────────────

variable "environment" {
  description = "Deploy environment (dev, staging, prod). Used for behavioral config (cache TTLs), NOT in resource names."
  type        = string
  default     = "dev"
}

variable "tags" {
  description = "Additional tags applied to all resources."
  type        = map(string)
  default     = {}
}
