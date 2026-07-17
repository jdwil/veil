# Extensions on AWS (EXT-11)

| Field | Value |
|-------|-------|
| **Status** | Adapters declared in VEIL; dual-loop default is File + `veil_local_fs` |
| **Env switch** | `VEIL_EXTENSIONS_BACKEND=file` (default) \| `ddb` |

## Adapters (same ports)

| Port | Local (default) | AWS |
|------|-----------------|-----|
| `ExtensionRegistry` | `FileExtensionRegistry` | `DdbExtensionRegistry` |
| `ExtensionSourceStore` | `FileExtensionSourceStore` | `S3ExtensionSourceStore` |
| `ExtensionArtifactStore` | `FileExtensionArtifactStore` | `S3ExtensionArtifactStore` |
| `ExtensionExecutor` | `FileExtensionExecutor` | (same File executor or future Lambda) |

Domain types (`ExtensionRecord`, `ExtensionVersion`) are **unchanged** across backends.

## DynamoDB item shape (registry)

Table: `EXTENSIONS_TABLE` (env).

| Attribute | Type | Notes |
|-----------|------|--------|
| `id` | S | `extension_id` UUID |
| `sk` | S | `meta` for record, `v#N` for version N |
| `payload` | S | JSON of `ExtensionRecord` or `ExtensionVersion` |
| `tenant_id` | S | optional GSI for tenant list |
| `scope` | S | Platform \| Product \| Tenant |

GSI suggestions: `tenant_id-index`, `scope-product-index`.

## S3 layout (source + artifacts)

Bucket: `EXTENSIONS_BUCKET`.

```text
src/{extension_id}/…           # package files (one logical repo per extension)
artifacts/{extension_id}/{version}/{target}
```

Default: **one package (git/logical repo) per extension**. Platform stock may share a monorepo as a packaging convenience only — registry entries remain per package.

## Dual-loop → AWS migration

1. Export local `VEIL_EXTENSIONS_DIR` JSON files (records + `versions/` + `src/`).
2. Create Dynamo table + S3 bucket (or LocalStack).
3. Set `VEIL_EXTENSIONS_BACKEND=ddb`, `EXTENSIONS_TABLE`, `EXTENSIONS_BUCKET`, AWS credentials.
4. Run import (host tool or one-shot bus `CreateExtension` / `save_version` / source writes).
5. Pin production `ExtensionRef.version` integers; never float `latest`.

## LocalStack

Use the same adapter code with:

```bash
export VEIL_EXTENSIONS_BACKEND=ddb
export AWS_ENDPOINT_URL=http://127.0.0.1:4566
export EXTENSIONS_TABLE=extensions
export EXTENSIONS_BUCKET=veil-extensions
```

Contract tests should assert port methods against LocalStack when credentials/endpoint are present; default CI remains **file** backend without AWS.

## Git-on-S3

Source trees live under `src/{id}/` in the extensions bucket. A git-compatible remote (git-s3 bridge) can treat each `src/{id}` prefix as a repo; runtime still addresses packages by **extension_id + integer version**, not branch names, for invoke.