# VEIL Architecture Decisions

This document captures architectural decisions made during development.
Agents working on VEIL should read this to understand WHY things are
structured the way they are.

---

## File Types and Their Purposes

| Extension | Purpose | Contains |
|-----------|---------|----------|
| `.layer` | Teaches vocabulary | Construct/statement definitions. Never contains domain logic. |
| `.veil` | Application code | Domain models, ports, adapters, services. Can be `sol` (app) or `pkg` (library). |
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

**VEIL generates:**
- All domain types (structs, enums, value objects)
- All trait definitions (ports)
- All adapter implementations
- All application services (async functions)
- Deps struct (dependency injection container)
- Workspace Cargo.toml with correct dependencies

**The runtime provides:**
- Concrete Bus implementation (InProcessBus, HttpBus, etc.)
- `main()` entry point (constructs deps, starts server)
- HTTP interface to expose the Bus
- Operational concerns (health checks, shutdown, observability)

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
