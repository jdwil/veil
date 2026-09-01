# Execution Topology — Shared Host vs. Dedicated Executor

**One engine, two topologies.** A VEIL project's execution artifact(s) run in
the shared **VEIL Execution Host** (default) *or* in the project's **own scoped
executor** — but both are the *same* `veil-runtime` in the *same*
`VEIL_ROLE=execution-host` run-mode. A dedicated executor is not a fork; it is
the same engine deployed as its own ECS service (`veil-<slug>-executor`) with
its artifact set scoped to one project and its own scaling policy.

See also: mind-palace `veil-execution-host-design` (topology section),
[`COMPILE_PIPELINE.md`](COMPILE_PIPELINE.md).

## The `[execution]` switch

Declared in a project's `veil.toml`, parsed at deploy/registration time
(alongside `[[triggers]]` and `[deploy.contribution]`):

```toml
[execution]
mode = "shared"        # default: register into the shared Execution Host
# or:
mode = "dedicated"     # deploy this project's own scoped executor service

[execution.dedicated]  # only honoured when mode = "dedicated"
cpu = 512
memory = 1024
min_tasks = 1
max_tasks = 4
autoscale_target_cpu = 60
```

- **Absent `[execution]`**, or `mode = "shared"` → the artifact registers into
  the shared host (no new infra). This is the default and correct choice for
  the overwhelming majority of projects.
- **`mode = "dedicated"`** → deploy provisions a scoped `veil-<slug>-executor`
  ECS service on the shared `veil-cluster` (Terraform, host-owned infra), hosting
  ONLY this project's artifact(s), with its own autoscaling policy from
  `[execution.dedicated]`. Triggers for this project route to THIS executor.
- An unknown `mode` is a **hard error** at registration (a typo like
  `mode = "dedcated"` fails loudly rather than silently mis-routing).

The resolved topology is persisted on each trigger row so the fire-routing path
invokes the RIGHT executor (shared host vs. the scoped service URL).

## When to go dedicated

**Default is shared.** Choose dedicated only when the artifact is one or more of:

| Reason | Why shared is a poor fit |
|--------|--------------------------|
| **Heavy** — high CPU/memory | Its sizing would blow up shared-host task sizing for *every* co-tenant artifact. Give it its own task with its own CPU/memory. |
| **Hot** — spiky, high-volume trigger | It needs its own scaling curve; on the shared host its bursts would scale (and pay for) the whole aggregate, and its noise degrades neighbours. |
| **Risky** — crash-prone or untrusted native code | A segfault/abort in FFI kills the WHOLE shared host and every artifact in it. `catch_unwind` catches Rust *panics*, NOT aborts/segfaults. Isolate the blast radius in its own process. |
| **Isolated** — tenancy/compliance requires physical separation | Some data-handling or contractual constraints require the artifact not share an address space or task with others. |
| **Toolchain-divergent** — needs a different `rustc`/deps than the shared host's pinned toolchain | Rust has no stable ABI; the shared host refuses to `dlopen` an artifact whose toolchain fingerprint drifts from its own. A dedicated executor MAY pin a different toolchain — but it must be internally consistent with ITS artifact (host image ↔ artifact toolchain match still applies). |

If none of these hold, stay shared. Dedicated means an extra ECS service to run,
scale, and pay for.

## Shared-host autoscaling (a requirement, not a nice-to-have)

The shared `veil-execution-host` ECS service **autoscales in all environments**
with target-tracking on **both CPU and memory**.

### Why memory tracking is load-bearing

Every HOT artifact's cdylib is `dlopen`'d **into the host's own address space**
(the `FfiLibraryCache`, an LRU keyed by content hash). So the host's resident
memory grows with the number of **distinct hot artifacts**, not merely with
request concurrency. Pure CPU tracking under-scales an aggregate-artifact-heavy
host; the memory target-tracking policy scales on that resident pressure.

### The memory / artifact relationship

```
task_memory  ≳  base_runtime + (ffi_cache_capacity × avg_cdylib_resident)
```

Two independent bounds keep this in check:

- **`ffi_cache_capacity`** (`VEIL_FFI_CACHE_CAPACITY`, default 64) — the MAX
  number of cdylibs kept resident. LRU evicts the rest; an evicted artifact is
  re-`dlopen`'d on next use, paying the warm-up cost again. Lower = less memory,
  more cold-load churn.
- **`task_memory` + the memory autoscaler** — when aggregate resident pressure
  pushes memory utilization past the target (80%), the service scales OUT (more
  tasks, each with its own cache) rather than OOM-killing.

Rule of thumb: **raise `ffi_cache_capacity` together with `task_memory`.** Raising
capacity without memory headroom trades eviction churn for OOM risk.

### Scaling bounds and cooldowns

- `autoscaling_min_capacity` / `autoscaling_max_capacity` are configurable
  (defaults min 1, max 4).
- Scale-in cooldown is generous (300s) to **avoid thrash**: scaling OUT lands a
  task with a *cold* `FfiLibraryCache` that must re-fetch and re-`dlopen` hot
  artifacts (warm-up cost), so we scale in slowly. Scale-out cooldown is short
  (60s) to react to bursts.

Dedicated executors get the same CPU + memory target-tracking, sized from their
`[execution.dedicated]` block (and typically a smaller `ffi_cache_capacity` since
they host a small, focused artifact set).

## Infra locations

- Shared host: `dlx-core/infra/locations/services/veil-execution-host/main.tf`
  (reuses `modules/veil-runtime`).
- Dedicated executor terraform is **generated** by `veil-runtime`
  (`crates/veil-runtime/src/dedicated_executor.rs`) per project slug, reusing the
  same `modules/veil-runtime` — same substrate, scoped service.

> **`terraform apply` is medium-high risk** (new ECS service, IAM, ALB on a
> shared cluster). Authoring/plan is safe; apply is gated on human confirmation.
