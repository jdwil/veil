# Deploy environments & existing infrastructure

veil-runtime provisions into **named environments** (dashlx: `dev`, `staging`, `prod`).  
Project detail shows provision **status per environment** via a selector.

## Standard service stack naming (`[deploy.stack]`)

DashLX products use a shared shape (not Terraform — still TOML intent in `veil.toml`):

| Resource | Default AWS name |
|----------|------------------|
| Lambda API | `veil-{service}-api` |
| Lambda consumer | `veil-{service}-consumer` |
| SQS (+ DLQ) | `veil-{service}` / `veil-{service}-dlq` |
| SNS topic | `veil-{service}` |
| DynamoDB table | `veil-{service}` |

Consumer Lambdas default to **900s (15 min)** timeout unless overridden.  
Lambdas get **VPC** from `[deploy.network]` (`vpc`, `security_groups` by name, `subnets` class).  
API Lambdas get an **API Gateway HTTP API trigger** (routes + resource policy / AddPermission).

```toml
[deploy]
resource_prefix = "veil"
service = "relay"          # → base veil-relay

[deploy.stack]
# optional hard pin: name = "veil-relay"
[deploy.stack.dynamodb]
[deploy.stack.sns]
[deploy.stack.sqs]
[deploy.stack.lambda_api]
[deploy.stack.lambda_consumer]

[[deploy.units]]
name = "relay-api"
stack_role = "lambda_api"  # binds unit → stack.lambda_api name
```

`LocalFs.read_project_deploy` expands this into `stack.names` for the UI and provisioner.
`resource_prefix = "veil"` avoids collisions with legacy (non-veil) service stacks.

### Config overrides (memory, timeouts, …)

Precedence: **unit > stack > hard default**.

```toml
[deploy.stack.lambda_api]
memory_mb = 512

[[deploy.units]]
name = "relay-api"
stack_role = "lambda_api"

[deploy.units.lambda]
memory_mb = 1024          # wins over stack
timeout_seconds = 30
architecture = "arm64"
```

Same for consumer: `[deploy.units.lambda]` or `[deploy.stack.lambda_consumer]`.

### SQS → consumer (event source mapping)

For `type = "lambda-consumer"`, provision:

1. Ensures SQS (+ DLQ) named `stack.names.sqs`
2. **`CreateEventSourceMapping`** so the queue is the Lambda’s event source

```toml
[deploy.units.queue]
batch_size = 1
event_source = true           # default true for consumers
# event_source_enabled = true
```

Set `event_source = false` only if you wire the queue elsewhere.

## Config file

Search order:

1. `VEIL_DEPLOY_CONFIG` (absolute path)
2. `~/.veil/deploy.toml`
3. `runtime/config/deploy.toml` (repo default)

```toml
default = "dev"

[env.dev]
region = "us-west-2"
account_id = "111122223333"
# Cross-account: runtime assumes this role when provisioning into dev
assume_role_arn = "arn:aws:iam::111122223333:role/veil-provision"
# external_id = "shared-secret"

# Logical name from project veil.toml → fuzzy patterns (* and ?)
[env.dev.gateways]
dashlx-services = ["dlx-rust-*", "*-dev-service-api"]

[env.staging]
region = "us-west-2"
assume_role_arn = "arn:aws:iam::444455556666:role/veil-provision"
[env.staging.gateways]
dashlx-services = ["dlx-rust-*", "*-staging-service-api"]

[env.prod]
region = "us-west-2"
assume_role_arn = "arn:aws:iam::777788889999:role/veil-provision"
[env.prod.gateways]
dashlx-services = ["dlx-rust-*", "*-prod-service-api"]
```

Env vars (optional):

| Variable | Purpose |
|----------|---------|
| `VEIL_DEFAULT_ENVIRONMENT` | Default when UI omits selection |
| `VEIL_ASSUME_ROLE_DEV` / `_STAGING` / `_PROD` | Role ARNs if not in TOML |
| `VEIL_ASSUME_EXTERNAL_ID` | STS external id for all roles |
| `VEIL_GW_<LOGICAL>` | Hard pin ApiId for a logical gateway |
| `VEIL_DEPLOY_EXECUTOR=mock` | Skip AWS, record DDB only |

## API Gateway policy

**Never create** a new API Gateway. Product units declare a logical name:

```toml
[deploy.units.api_gateway]
gateway = "dashlx-services"
path_prefix = "/relay"
```

Resolution order:

1. `gateway_id` in project `veil.toml`
2. `VEIL_GW_DASHLX_SERVICES` (env)
3. SSM `/veil/{env}/gateways/dashlx-services`
4. **Fuzzy** match of platform patterns for that environment (e.g. `dlx-rust-*`)
5. Exact / fuzzy match of the logical name itself

If multiple HTTP APIs match a pattern, the **shortest name** is chosen (deterministic).

## APIs

| Method | Path | Notes |
|--------|------|--------|
| GET | `/api/deploy_environments` | Catalog for UI selector |
| GET | `/api/project_infras/{id}?environment=dev` | Hub veil.toml + env catalog |
| GET | `/api/deployment_status?environment=&unit_name=` | Status in selected env |
| POST | `/api/provision-project` | Body: `{ project_slug, environment }` |

## Cross-account

When `assume_role_arn` is set for an environment, `DeployExec` calls STS `AssumeRole` and uses those credentials for Lambda / API GW / SQS / (optional) DDB in that account.  
The runtime host credentials only need `sts:AssumeRole` on the target roles.
