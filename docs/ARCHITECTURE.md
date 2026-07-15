# VEIL Architecture Decisions

This document captures architectural decisions made during development.
Agents working on VEIL should read this to understand WHY things are
structured the way they are.

**Language pipeline rule (sugar):** any surface sugar change updates
**parser + typecheck + codegen + test** in the same PR — see
[`docs/ENGINE.md`](./ENGINE.md) (ACS-007).

---

## File Types and Their Purposes

| Extension | Purpose | Contains |
|-----------|---------|----------|
| `.layer` | Teaches vocabulary | Construct/statement definitions. Never contains domain logic. |
| `.veil` | Application code | Domain models, ports, adapters, services. `pkg <Name>` at the top. (`sol` is deprecated alias.) |
| `.stub` | External crate API | Declares shapes of third-party Rust crate types for type inference. |

**Key rule:** If it defines concrete entities, repos, or business logic, it's a `.veil` file.
If it defines keywords/abstractions that map to core shapes, it's a `.layer` file.

---

## Package Boundaries and the Expose Block

A `pkg` can define an `expose` block that declares its public API contract.
Consumers who `use <pkg_name>` get ONLY the exposed surface — not the
internal implementation.

The expose block contains:
- **`node <Name>`** — a single exposed operation (command or query)
- **`input`** — typed parameters the operation accepts
- **`output`** — typed response fields the operation returns
- **`cst`** — free-text constraints (e.g. `flow-only`)

```
expose
  node CreateCustomer
    desc "Register a new customer"
    input
      email: Email
    output
      customer_id: UUID
```

Internals (aggregates, repos, adapters, services) are PRIVATE to the package.

---

## CQRS Enforcement

Commands and queries are strictly separated:

**Commands:**
- Accept a client-generated UUID as the `id` field
- Do NOT return domain data (only `Res!` — success or failure)
- May return a correlation token for async status checking
- Represent intent to change state

**Queries:**
- Return typed DTOs (never internal aggregates)
- Have no side effects
- Can be cached, repeated safely

**Why UUIDs are passed in:** We never rely on the database to generate IDs.
The client generates the UUID upfront. This enables idempotency, optimistic
UI, and avoids round-trip dependencies.

---

## Inter-Context Communication

Bounded contexts NEVER import each other's types directly. All communication
goes through the **Bus**:

```
Context A                    Bus                     Context B
    |                         |                         |
    |-- request GetCohort --> |                         |
    |                         | -- calls handler -----> |
    |                         | <-- returns DTO ------- |
    | <-- CohortDTO --------- |                         |
```

The Bus is an abstract trait (`ddd.layer` declares it). The concrete
implementation is provided by `veil-runtime` at deployment time.

**Message format:** Messages are serialized as `serde_json::Value` payloads.
Commands/events carry a `"type"` field identifying the message name and a
`"target"` field identifying the recipient port/method. Results are returned
as JSON values, enabling orchestrators to index into them without depending
on another context's concrete Rust types.

---

## Layer Resolution Order

When VEIL encounters `use <name>`:
1. **Local directory** — same dir as the .veil file (highest priority)
2. **System layers** — `layers/` directory (ships with VEIL: base, ddd, functional)
3. **External resolver** — trait-based port for custom backends (databases, etc.)

For `.stub` files: local directory only (no system stubs).

Note: `crm.layer` lives in `examples/` as a composability proof. To use it
in a `.veil` file, place a local copy next to the file or reference it via
a custom resolver.

---

## Token Efficiency

VEIL is designed for AI-generated code. Every syntax choice prioritizes
minimal token count:

- No `call` keyword needed — bare `Target.method(args)` is a call
- `+` replaces `export` (1 char vs 6)
- `name!(params)` implies `-> Res!` (saves return type declaration)
- `Id` = UUID, `Dt` = DateTime (short aliases)
- Bare field names infer types by convention (`id` → UUID, `created` → DateTime)
- Indentation-based — no `{ } ;` noise

**Docs only show the terse forms.** The parser accepts verbose forms for
backward compatibility but they should never be used in new code.

---

## What VEIL Generates vs What the Runtime Provides

**VEIL generates (Rust target — default):**
- All domain types (structs, enums with data variants, value objects)
- All trait definitions (ports)
- All adapter implementations
- All application services (async functions)
- Deps struct (dependency injection container)
- Workspace Cargo.toml with correct dependencies
- manifest.json per context (deps, handlers, strategy)

**VEIL generates (TypeScript target — `veil gen -t ts`):**
- Typed interfaces for all structs (`export interface`)
- Typed interfaces for all ports (async method signatures)
- Async service functions
- Discriminated unions for data-carrying enums
- Project scaffolding (package.json, tsconfig.json)

**The runtime provides:**
- Concrete Bus implementation (InProcessBus, HttpBus, etc.)
- `main()` entry point (constructs deps, starts server)
- HTTP interface to expose the Bus
- Operational concerns (health checks, shutdown, observability)

---

## The manifest.json Contract

Each generated context crate includes a `manifest.json` that describes
everything veil-runtime needs to wire the application at startup. This is the
**only** handoff point between the VEIL compiler and the runtime — no runtime
code reads `.veil` files directly.

### Example manifest.json

```json
{
  "context": "IAAA",
  "crate": "iaaa",
  "deps": {
    "cohort_repo": {
      "trait": "CohortRepo",
      "adapter": "PgCohortRepo",
      "env": ["DATABASE_URL"]
    },
    "bus": {
      "trait": "Bus",
      "provided_by": "runtime"
    },
    "auth_service": {
      "trait": "AuthService",
      "provided_by": "runtime",
      "strategy": "cognito"
    }
  },
  "handlers": {
    "GetCohort": {
      "function": "handle_get_cohort",
      "inputs": [
        { "name": "id", "type": "Named(\"Id\")" }
      ]
    },
    "CreateList": {
      "function": "handle_create_list",
      "inputs": [
        { "name": "id", "type": "Named(\"Id\")" },
        { "name": "cohort_id", "type": "Named(\"Id\")" },
        { "name": "name", "type": "Named(\"Str\")" }
      ]
    }
  },
  "expose": []
}
```

### Field Reference

| Field | Purpose |
|-------|---------|
| `context` | The bounded context name (from the `ctx` construct) |
| `crate` | The generated Rust crate name (snake_case) |
| `deps` | All trait dependencies the context requires |
| `deps.<name>.trait` | The trait name (port interface) |
| `deps.<name>.adapter` | Concrete implementation struct (if defined in .veil) |
| `deps.<name>.env` | Environment variables the adapter needs |
| `deps.<name>.provided_by` | `"runtime"` if the runtime supplies this (Bus, Auth, etc.) |
| `deps.<name>.strategy` | Optional hint for runtime-provided deps (e.g. `"cognito"`, `"local"`) |
| `handlers` | All message handlers the context exposes via the Bus |
| `handlers.<MessageName>.function` | The generated Rust function name to call |
| `handlers.<MessageName>.inputs` | Typed parameter list (for deserialization) |
| `expose` | Public API contract (from the `expose` block, if present) |

### What veil-runtime Does With It

1. **Reads `deps`** → constructs each adapter (using env vars), builds the `Deps` struct
2. **Reads `handlers`** → registers each handler as a Bus message listener
3. **Reads `provided_by: "runtime"`** → injects its own implementations (Bus, AuthService)
4. **Reads `strategy`** → selects the appropriate runtime implementation variant
5. **Reads `expose`** (future) → generates API Gateway routes or Lambda entrypoints

### Design Principles

- The manifest is **declarative** — it describes WHAT is needed, not HOW to build it
- The VEIL compiler is **not aware** of runtime implementation details (Lambda vs ECS vs local)
- The runtime is **not aware** of domain semantics — it just wires traits to adapters
- Deployment topology (single Lambda vs multiple Lambdas vs monolith) is a **runtime decision** based on config, not a compiler decision

---

## Deployment Model

```
┌─────────────────────────────────────────────┐
│  veil-runtime (provides)                    │
│  - InProcessBus / HttpBus / SqsBus          │
│  - main() harness                           │
│  - HTTP server (axum)                       │
│  - Config loading                           │
│                                             │
│  VEIL-generated crates (domain code)        │
│  - dlx_core (IAAA context)                  │
│  - wear_test (WearTesting context)          │
│  - veil_shared (Bus trait, DomainError)     │
└─────────────────────────────────────────────┘
```

Each bounded context CAN be deployed as:
- A module in a monolith (InProcessBus)
- A separate service (HttpBus between contexts)
- A serverless function (LambdaBus)

The VEIL code is identical in all cases. Only the Bus implementation changes.

---

## Source Control and Storage (Future)

When deployed to DashLX:
- .veil/.layer/.stub files stored in git repos on S3
- Git metadata (diffs, tags, branches) cached in PostgreSQL/DynamoDB for fast access
- The visual editor reads from DB cache, writes trigger git commits
- Build service (ECS) clones from S3, runs `veil gen`, compiles with cargo
- Compiled artifacts cached in S3/ECR

VEIL itself has NO knowledge of storage — it's a pure compiler library.
The platform handles persistence.
