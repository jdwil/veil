# Storage adapters (RT-010 / RT-011 / RT-015)

Local-first defaults; cloud adapters selected by env. **No silent success** for
unimplemented providers.

| Role | Port | Local default | Cloud |
|------|------|---------------|-------|
| Objects / blobs | `ObjectStorage` | `FsObjectStore` | `S3ObjectStore` |
| Metadata | put/get/list by kind+id | `FileMetaStore` | `DdbMetaStore` (stub) |

## Env

| Variable | Default | Meaning |
|----------|---------|---------|
| `VEIL_STORAGE` | `fs` | `fs` or `s3` |
| `VEIL_DATA_DIR` | `~/.veil` | Root for fs objects + meta |
| `VEIL_S3_ENDPOINT` | — | Required for `s3` (e.g. LocalStack `http://127.0.0.1:4566`) |
| `VEIL_S3_BUCKET` | `veil` | Bucket name |
| `VEIL_S3_REGION` / `AWS_REGION` | `us-east-1` | Region |
| `VEIL_S3_PATH_STYLE` | `true` | Path-style URLs (LocalStack-friendly) |
| `VEIL_META` | `fs` | `fs` or `ddb` |
| `VEIL_DDB_TABLE` | — | Required for `ddb` |
| `VEIL_DDB_ENDPOINT` | — | Optional LocalStack endpoint |
| `VEIL_DDB_REGION` / `AWS_REGION` | `us-east-1` | Region |

## Selection

```rust
// crates/veil-local
let objects = veil_local::object_store_from_env()?;
let meta = veil_local::meta_store_mode_from_env()?;
```

### S3 MVP

Minimal REST via `curl` (path-style). Suitable for LocalStack smoke tests.
Production AWS should move to a dedicated adapter package with SigV4 / SDK.

### DynamoDB

`DdbMetaStore::from_env()` validates config, then **every op returns
`StorageError::NotImplemented`** with table/region in the message. This satisfies
“explicit not_implemented” until the AWS SDK path lands — deployers never get a
false green.

## Does not replace local defaults

`veil serve`, compile pipeline, and agent paths keep disk + `FileMetaStore`
unless env overrides are set.
