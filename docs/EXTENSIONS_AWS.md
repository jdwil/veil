# Extensions on AWS (EXT-11)

| Field | Value |
|-------|-------|
| **Status** | **Done** — same VEIL adapters call `ExtStore`; AWS lives in `runtime/ext_store` |
| **Env switch** | `VEIL_EXTENSIONS_BACKEND=file` (default) \| `ddb` |

## Permanent architecture (no raw SDK in VEIL)

```text
runtime.veil adapters (File* / Ddb* / S3*)
        │
        ▼  veil_ext_store.stub
runtime/ext_store  (veil_ext_store crate)
        │
        ├── file backend  →  VEIL_EXTENSIONS_DIR JSON / src / artifacts
        └── ddb backend   →  DynamoDB (meta) + S3 (src/artifacts)
```

**Never** call `DdbClient` / `S3Client` from VEIL adapter bodies for Extensions.
Raw AWS SDK lowering does not match real SDK builders (`put_item`, attribute maps, …).
The facade owns correct SDK calls behind the `aws` Cargo feature.

| Port | Adapter (VEIL) | IO |
|------|----------------|----|
| `ExtensionRegistry` | `FileExtensionRegistry` (dual-loop) or `DdbExtensionRegistry` | `ExtStore.put/get/list_record(s)` + versions |
| `ExtensionSourceStore` | `FileExtensionSourceStore` / `S3ExtensionSourceStore` | `ExtStore.*_source` / `package_root` |
| `ExtensionArtifactStore` | `FileExtensionArtifactStore` / `S3ExtensionArtifactStore` | `ExtStore.put_artifact` / `get_artifact_uri` |
| `ExtensionExecutor` | `FileExtensionExecutor` | in-process success path (local); Lambda later |

Domain types (`ExtensionRecord`, `ExtensionVersion`) are **unchanged** across backends.

Dual-loop host always wires **File\*** via `extensions_deps()`; set
`VEIL_EXTENSIONS_BACKEND=ddb` to flip the same ExtStore API onto AWS without
rewriting product code. Ddb*/S3* adapters are the same ExtStore calls for
explicit deploy wiring if preferred.

## DynamoDB item shape (registry)

Table: `EXTENSIONS_TABLE` (env, default `extensions`).

| Attribute | Type | Notes |
|-----------|------|--------|
| `id` | S | `extension_id` UUID |
| `sk` | S | `meta` for record, `v#N` for version N |
| `payload` | S | JSON of `ExtensionRecord` or `ExtensionVersion` |

Build with `veil_ext_store` feature `aws` for deploy images; default dual-loop is **file-only** (no AWS crates required for CI).

## S3 layout (source + artifacts)

Bucket: `EXTENSIONS_BUCKET` (default `veil-extensions`).

```text
src/{extension_id}/…           # package files (one logical repo per extension)
artifacts/{extension_id}/{version}/{target}
```

## Dual-loop → AWS migration

1. Export local `VEIL_EXTENSIONS_DIR` JSON files (records + `versions/` + `src/`).
2. Create Dynamo table + S3 bucket (or LocalStack).
3. Set `VEIL_EXTENSIONS_BACKEND=ddb`, `EXTENSIONS_TABLE`, `EXTENSIONS_BUCKET`, AWS credentials.
4. Ensure the host binary links `veil_ext_store` with `features = ["aws"]`.
5. Pin production `ExtensionRef.version` integers; never float `latest`.

## LocalStack

```bash
export VEIL_EXTENSIONS_BACKEND=ddb
export AWS_ENDPOINT_URL=http://127.0.0.1:4566
export EXTENSIONS_TABLE=extensions
export EXTENSIONS_BUCKET=veil-extensions
```

Default CI remains **file** backend without AWS (`runtime/scripts/extensions_smoke.sh`).

## Git-on-S3

Source trees live under `src/{id}/` in the extensions bucket. Runtime addresses
packages by **extension_id + integer version**, not branch names, for invoke.
