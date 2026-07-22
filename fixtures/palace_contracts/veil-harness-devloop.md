# veil-harness-devloop

**Type:** Concept  
**Summary:** How VEIL generates runnable applications via @main, the dual-loop dev workflow (gen + cargo + smoke), HTTP route generation, multi-package harness, and the veil.toml target configuration.  
**Links:** veil-project-index, veil-di-layer, veil-editing-patterns, veil-codegen-targets-vs-layers

## App Harness Overview

VEIL-authored harnesses replace handwritten Rust bootstrap. Use `@main` / `@pvd` / `@dep` (from `di.layer`) to compose a runnable program.

**Minimal demo:**
```bash
veil gen examples/local_run.veil -o /tmp/local_run -t rust
cd /tmp/local_run && cargo run -p veil_bin
```

`@main` packages emit `crates/veil_bin` with:
1. InProcessBus construction (from veil_shared)
2. Memory adapters for ports
3. Calls generated application services

Library-only packages (no `@main`) generate context crates without `veil_bin`.

## Generated Workspace Layout

```
Cargo.toml              # workspace
crates/veil_shared/     # Bus trait, DomainError, shared types
crates/<context>/       # domain + application + infrastructure
crates/veil_bin/        # [[bin]] with main.rs harness
  src/main.rs
```

**Host modes:**
- **App harness** — VEIL `@main` constructs everything (local run, custom deploy)
- **Host harness** — External host reads `manifest.json`, injects `provided_by: "runtime"` deps

## HTTP Route Generation

**Preferred:** `@route("METHOD /path")` annotation on handler:
```
@route("GET /api/users/{id}")
handler HandleGetUser
  ...
```

**Fallback** (legacy, when no @route): name-derived routes:
| Name pattern | Method | Path |
|-------------|--------|------|
| `ListThings` | GET | `/api/things` |
| `GetThing` | GET | `/api/things/{id}` |
| `CreateThing` | POST | `/api/things` |
| `UpdateThing` | PUT | `/api/things/{id}` |
| `DeleteThing` | DELETE | `/api/things/{id}` |

- List inputs from **query string** (not random UUIDs)
- Create/Update inputs from JSON body
- Handlers call generated `fn`s with wired Deps

## Dual-Loop Dev Workflow

The agent closed loop:
```
write_source → gen + cargo check (smoke)
    ├─ fail → WRITE REJECTED, restore previous → dev_logs → fix
    └─ ok   → list_routes / read_generated → dev_restart → http_request
```

**Smoke** is on by default. After WRITE REJECTED: MUST call `dev_logs` / `smoke_status` before rewriting.

**Auto-restart** after successful smoke (default on).

**Smoke scope (multi-package):** checks only the primary context crates for the changed file, not the whole workspace.

## veil.toml Targets

```toml
[[targets]]
name = "backend"
package = "wear_test.veil"
target = "rust"
output = "generated/backend"
dev_command = "cargo run -p veil_bin"
dev_port = 3000

[[targets]]
name = "frontend"
package = "wear_test_ui.veil"
target = "typescript"
output = "generated/frontend"
dev_command = "npm install && npx vite dev --port 5174"
dev_port = 5174

[dev]
# Additional packages for multi-package local harness
packages = ["/path/to/dlx_core/dlx_core.veil"]
```

DevToolbar: per-target play/stop or **All targets**.
On start: `veil gen <package> -t <target> -o <output>` then spawn `dev_command`.

## Multi-Package Harness

When `[dev].packages` is non-empty (Rust target):
1. `veil gen <primary> --no-prune`
2. `veil gen <each dev package> --no-prune`
3. `veil gen-harness <primary> <dev...>` — one combined `veil_bin`
4. Prune crates not needed; merge workspace

Combined `veil_bin` wires ALL context crates' adapters and merges REST routes into one axum server.

## Frontend Proxy (@proxy)

UI packages call relative `/api/*`. Vite proxies to backend `dev_port`.

```veil
pkg MyUi
  use sveltekit5
  @proxy("/api", "http://127.0.0.1:3000")
  app MyApp
    ...
```

`@proxy` MUST be placed **before** the `app` construct (leading annotation). Codegen via `sveltekit5.layer` template emits `vite.config.ts` proxy config.

**Bus vs REST:** Bus is backend IPC only. Product UI uses HTTPS + Vite proxy. Never route browser traffic through the Bus.

**Source of truth:** `docs/HARNESS.md`, `docs/AGENT.md`, `fixtures/multi_harness/`
