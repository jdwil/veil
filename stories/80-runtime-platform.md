# Runtime Platform Stories

## Product decision (locked)

### Two local experiences (both valuable)

| Experience | Tool | Purpose |
|------------|------|---------|
| **Project IDE** | `veil serve` in the **project root** | Primary coding surface: graph review, edit, diagnostics, agent prompt (soon). This is the default “fire it up and code” path. |
| **Local platform runtime** | veil-runtime with **filesystem + sqlite** (or similar) | Optional local **build / source / package** environment — run platform services on a laptop without cloud. Useful for dogfooding the platform and for agent/platform features that need storage. |

These are complementary, not mutually exclusive. A coder may only use the IDE.
A platform developer (or advanced user) may also run local runtime.

### Cloud adapters are pluggable, not AWS-shaped core

- **LocalStack / real AWS** = only for testing the *AWS* deploy path before
  shipping to AWS.
- They do **nothing** for GCP/Azure/other providers later.
- Ports (`ObjectStorage`, `MetadataStore`, …) stay abstract; adapters are
  per-provider packages/layers.
- **Default local adapters:** filesystem + sqlite (or file DB) — first-class,
  not a second-class “mock.”

### Depends on

- GEN-002 (adapter lowering actually works)
- Harness path RT-000–003 (something can run the services)
- Dual-loop P0s still outrank this file

---

## RT-010: Filesystem object storage adapter (local-first)

**Status:** Open · **Priority:** P2  
**As a** local platform user  
**I want** `ObjectStorage` backed by the filesystem  
**So that** runtime works offline without S3

**Acceptance criteria:**

- `put` / `get` / `list` / `delete` on a configurable root directory
- Used as default when `VEIL_STORAGE=fs` or no cloud creds
- No `todo!` for these methods
- Unit tests with temp dirs

---

## RT-011: Sqlite (or file) metadata store (local-first)

**Status:** Open · **Priority:** P2  
**As a** local platform user  
**I want** `MetadataStore` on sqlite/file  
**So that** repos/branches/artifacts persist locally

**Acceptance criteria:**

- CRUD for entity kinds used by Storage services
- Default local path under project or `~/.veil/`
- Schema migration strategy documented (even if MVP is “wipe ok”)
- Env naming aligned with LoadConfig (`VEIL_*`)

---

## RT-012: Content addressing (real hashes)

**Status:** Open · **Priority:** P2  
**As a** platform  
**I want** cryptographic content hashes  
**So that** artifacts are not `content.len()`

**Acceptance criteria:**

- sha2 (or documented algorithm) used for content-addressed paths
- Stable across runs; tests included

---

## RT-013: Compile pipeline MVP (local)

**Status:** Open · **Priority:** P2  
**As a** local platform / agent  
**I want** “compile this package” to run `veil gen` + target build on the machine  
**So that** ArtifactMetadata reflects a real local build

**Acceptance criteria:**

- Invokes configurable `veil` + `cargo`/`tsc` paths
- Captures logs, success/failure, artifact location
- This is the **local build environment** story — not Lambda yet
- Failures are structured errors

**Synergy:** Complements project-root `veil serve` (IDE) with heavier
compile-as-a-service for multi-package/platform flows.

---

## RT-014: Provider-agnostic deploy ports + AWS adapter later

**Status:** Open · **Priority:** P3  
**As a** multi-cloud product  
**I want** deploy expressed as ports  
**So that** AWS is one adapter, not the architecture

**Acceptance criteria:**

- `DeployTarget` / deploy port does not hardcode Lambda-only semantics in core
- AWS adapter (Lambda/ECR/…) is optional package; LocalStack test path documented
- Explicit `not_implemented` for providers without adapters
- No success status without a real action

---

## RT-015: S3 / DynamoDB adapters (AWS path only)

**Status:** Open · **Priority:** P3  
**As an** AWS deployer  
**I want** real S3/DDB adapters for pre-prod testing  
**So that** I can LocalStack or AWS-integration-test before deploy

**Acceptance criteria:**

- Same ports as RT-010/011; different adapter impls
- Feature/env selection: `VEIL_STORAGE=s3` etc.
- Does not replace local fs+sqlite defaults
- GEN-002 fixed so bodies are not `todo!("SQL: …")`

---

## RT-016: VCS model decision (gix vs object store)

**Status:** Open · **Priority:** P2  
**As an** architect  
**I want** an explicit local-source model  
**So that** Storage services stop half-modeling git and S3 keys

**Related decision (agent/IDE):** Local runtime keeps **package source files on
disk**; **sqlite holds metadata only** (repos, branches, artifacts). See
[100-ide-agent.md](100-ide-agent.md) SourceStore matrix. Do not default to
storing full source trees in sqlite.

**Acceptance criteria:**

- ADR: pure content-addressed object store vs real git (gix)
- Local IDE may keep using the user’s real git in the project root;
  platform storage is separate unless we unify intentionally
- Align with SourceStore adapters (AGT-004)
- Remove unused stubs or implement the chosen path
- GetDiff real or removed from API

---

## RT-017: Daemon / agent surface honesty

**Status:** Open · **Priority:** P3  
**As an** IDE/agent client  
**I want** WS/agent features real or clearly unavailable  
**So that** we do not no-op with success

**Acceptance criteria:**

- Primary agent UX is **AGT-001+** on project-root `veil serve` (see
  [100-ide-agent.md](100-ide-agent.md)), not a fake daemon agent string
- Platform `/ws` only if still needed for remote sessions (AGT-010); else drop
  claims from UI
- No fake LLM success without a model call

---

## RT-018: `runtime-ui` against real local APIs

**Status:** Open · **Priority:** P3  
**As a** platform user  
**I want** control-plane UI talking to local runtime  
**So that** the mockup becomes operable

**Acceptance criteria:**

- Gen target in Makefile
- Routes match local harness/daemon
- Health + list packages/manifests happy path

---

## RT-019: `sol` → `pkg` for runtime sources

**Status:** Open · **Priority:** P2  
**As a** maintainer  
**I want** modern `pkg` keyword only  
**So that** deprecated aliases fade out

---

## RT-020: Project-root workflow is the default story

**Status:** Open · **Priority:** P1  
**As a** coder  
**I want** docs and tooling that say: open project → `veil serve` → code  
**So that** local platform runtime is opt-in power, not a gate

**Acceptance criteria:**

- README / MISSION / runtime README describe project-root IDE as default
- Optional: `veil serve` discovers `.veil` in cwd, no special platform required
- Agent prompt story attaches here (PAR-009 / future UX), not only to AWS runtime
- Local runtime documented as “when you need platform services locally”
