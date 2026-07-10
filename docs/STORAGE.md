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

### S3 (RT-025)

In-process HTTP via `reqwest` (RT-026). Path-style by default (LocalStack).
When `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` (or `VEIL_S3_*`) are set,
requests are signed with **AWS SigV4**.

### DynamoDB (RT-024)

`DdbMetaStore` uses DynamoDB JSON HTTP API (`PutItem` / `GetItem` / `DeleteItem`
/ `Scan`). Set `VEIL_DDB_ENDPOINT` for LocalStack. Items store base64 payload
under `pk = kind#id`. Failures are structured HTTP errors (never silent success).

### HTTP (RT-026)

`veil_local::http::request` — blocking client, timeouts; **no curl**.
`RemoteHttpProvider` uses the same stack.

## Does not replace local defaults

`veil serve`, compile pipeline, and agent paths keep disk + `FileMetaStore`
unless env overrides are set.
