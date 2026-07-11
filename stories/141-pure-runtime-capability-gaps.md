# Completing pure VEIL runtime — remaining work & missing capabilities

**Parent epic:** [140-pure-veil-runtime.md](140-pure-veil-runtime.md)  
**Question:** What does it take to finish “runtime pure VEIL front+back, full
functionality”? Which gaps need **new VEIL capabilities** (language/codegen/host
ports) vs ordinary product wiring?

---

## Honest completion status

| Layer | MVP today | Remaining to “pure + full” |
|-------|-----------|----------------------------|
| Multi IDE kernel | Live (`veil-server`) | Keep as residual non-VEIL (allowed) |
| Projects hub API | Live | Done |
| Bus List/Create/R/W/git/compile/deploy | Live in **Rust `platform.rs`** | Must move behind **generated** handlers or VEIL ports |
| Shell UI | Hand HTML implementing VEIL screens | **Generated SPA** from `runtime-ui.veil` is primary |
| Host process | Handwritten bootstrap (~400 lines) | **≤50-line trampoline** + generated `@main` |
| Agent | Hint → IDE agent route | Full turn from shell Agents page |
| Auth/cloud multi-tenant | Out of scope | PVR-042 |

**You cannot finish “pure VEIL” only by writing more runtime.veil** until the
items in §2 land. Those are **engine capabilities**, not more domain code.

---

## Effort overview

| Track | Est. effort | Depends on |
|-------|-------------|------------|
| **A. Language/codegen gaps** (§2) | 2–4 weeks focused | — |
| **B. Host from `@main` + wire generated storage** | 1–2 weeks after A.1–A.3 | A |
| **C. Bundle & serve runtime-ui SPA** | 1–2 weeks after A.4–A.5 | A |
| **D. Agents + polish + delete HTML** | ~1 week | B, C |
| **Total critical path** | **~5–8 weeks** (one senior full-time) | |

Parallelize: A.4 (UI emit) || A.1–A.3 (host emit); B || C once A lands.

---

## 1. Missing VEIL capabilities (must add)

These block pure authorship. Each is a **capability story** (engine), not app code.

### CAP-001: External crate / package linkage from generated Rust — Done · P0

**Problem:** Generated `@main` / packages cannot declare a Cargo dependency on
`veil-server`, `veil-local`, or other monorepo crates. Host cannot call
`build_multi_router` from VEIL without a handwritten bridge.

**Need**

```veil
pkg VeilRuntimeHost
  use ddd
  use di
  link veil_server
  link veil_local path "../../crates/veil-local" features "local"
```

**Acceptance**

- [x] Layer or core syntax: declare external Cargo deps (path + crate name + features).
- [x] Codegen emits `[dependencies]` entries in generated `Cargo.toml`.
- [x] Generated code can `use veil_server::…` in `@main` / adapters.
- [x] Documented security: only allowlisted crates (or path deps under workspace).

**Done notes (CAP-001)**

- Core keyword `link` (lexer + package/solution parse).
- AST: `LinkDecl` on `Package` / `Solution`; serializer round-trips.
- Codegen: `crates/veil-codegen/src/links.rs` resolves allowlist + relative paths;
  emits workspace path deps and `name.workspace = true` on shared / modules / `veil_bin`.
- Docs: `docs/LANGUAGE.md` § `link`.
- Tests: parser `test_parse_link_decls`, codegen `link_external_crates_in_cargo_toml`,
  unit tests in `links.rs`.

**Mission impact:** Without this, host always stays handwritten or only talks
HTTP to a second process.

---

### CAP-002: Host HTTP surface as VEIL construct / port — Todo · P0

**Problem:** Serving static files, nesting routers, and binding a port are only
in Rust bootstrap. Runtime packages cannot express “mount multi IDE + shell + bus”.

**Need (pick one design; implement one)**

| Option | Idea |
|--------|------|
| **A. Port** | `provided_by: runtime` port `HttpHost` with methods `mount_ide()`, `serve_dir()`, `listen(port)` implemented by trampoline |
| **B. Construct** | `svc HttpServer` / layer `http.layer` with `route`, `static`, `listen` lowered to axum |
| **C. Template section** | Codegen section `host_http` filled by layer templates calling known Rust helpers |

**Recommended:** **A** short-term (thin port, no new grammar), **B** long-term.

**Acceptance**

- [ ] VEIL can express: listen on port, serve SPA dir, mount IDE multi router,
      mount bus JSON routes.
- [ ] Generated host uses that expression; bootstrap only constructs the port.
- [ ] Example: `runtime/src/host.veil` becomes real `@main` using the port.

---

### CAP-003: Auto-register Bus handlers from manifest — Todo · P0

**Problem:** Generated crates expose handlers, but the host must hand-register
names. Live platform logic was reimplemented in `platform.rs` instead of calling
generated `storage`/`tools`.

**Need**

- Codegen emits a `register_all(bus: &mut dyn Bus)` (or map of name → fn) per
  package / workspace.
- Host `@main` calls `register_all` once.
- Manifest lists handler names (already partially true).

**Acceptance**

- [ ] `veil gen` produces `register_handlers` module for multi-crate workspaces.
- [ ] Runtime host uses generated registration for Storage/Tools (no parallel
      hand-written dispatch table for those ops).
- [ ] Integration test: gen fixture package → register → invoke by name.

---

### CAP-004: System ports (FS / process / git) as `provided_by` adapters — Todo · P1

**Problem:** File and git ops live in handwritten `platform.rs`. Pure VEIL
storage services need injectable ports without inventing magic in the typechecker.

**Need**

```veil
# already partially exist as ports in runtime.veil — ensure codegen + host inject
port FileSystem
  read(path: Str) -> Res!<Str>
  write(path: Str, content: Str) -> Res!
  list(prefix: Str) -> Res!<List<Str>>

port GitRepo
  branches() -> Res!<List<Str>>
  …
```

Host (or `veil-local`) provides default local impls via DI / `provided_by`.

**Acceptance**

- [ ] Generated storage services depend on ports, not raw `std::fs` in VEIL.
- [ ] Local defaults wired in host trampoline once (not per-handler).
- [ ] Tests use temp-dir adapter.

---

### CAP-005: UI emit = browser-ready SPA (not TS fragments only) — Todo · P0

**Problem:** `veil gen -t typescript` for `runtime-ui.veil` emits `src/index.ts`
+ package.json, **not** a runnable browser app. Product shell stays hand HTML.

**Need**

| Piece | Requirement |
|-------|-------------|
| Target | `svelte` or `typescript` + **bundled** `dist/` (Vite/esbuild invoke) |
| Entry | `index.html` + hydrated app from pages/layouts/comps |
| Routes | `@route` → client or SvelteKit file tree |
| Assets | CSS from `style` raw blocks |
| API | Fetch relative `/api/…` (same origin) |

**Acceptance**

- [ ] `veil gen runtime-ui.veil -t svelte` (or ts+bundle) produces `dist/` openable
      via `file` or static server with no hand HTML product logic.
- [ ] `make pure-runtime-build` copies `dist/` to host static root.
- [ ] `GET /` serves that dist as primary UI.
- [ ] Round-trip demo: dashboard lists projects from live API.

**Mission impact:** Unblocks pure front-end claim.

---

### CAP-006: Bin crate layout for multi-package runtime workspace — Todo · P1

**Problem:** RT-021 — large `runtime.veil` gen is multi-crate workspace without a
single clean `veil_bin` that links IDE kernel + all contexts.

**Acceptance**

- [ ] Gen emits runnable bin member that depends on all context crates + optional
      external links (CAP-001).
- [ ] `cargo run -p veil_bin` from gen output starts server when `@main` present.
- [ ] Documented for monorepo `runtime/` layout.

---

### CAP-007: Config PATCH / runtime settings port — Todo · P2

**Problem:** Shell Config page needs write API; only GET config exists.

**Acceptance**

- [ ] `PATCH /api/config` or Bus `SaveConfig` updates allowlisted keys
      (`projects_dir` with validation).
- [ ] VEIL UI binds to it.

---

## 2. Product wiring (no new language — after CAP-*)

| Story | Work | After |
|-------|------|--------|
| PVR-010 true | Rewrite host as VEIL `@main` using CAP-001/002/003 | CAP-001–003 |
| PVR-011 true | Delete parallel `platform.rs` dispatch; call generated storage | CAP-003–004 |
| PVR-021–023 | Drop hand HTML; only gen SPA | CAP-005 |
| PVR-016 | Agents page → real agent turn | CAP-002 routes or existing multi API |
| PVR-032 | Delete bootstrap static product HTML | CAP-005 + PVR-023 |
| PVR-031 CI | Smoke pure-runtime in CI | All above |

---

## 3. Recommended delivery plan

### Sprint 1 — Unblock host (CAP-001, CAP-002, CAP-003)

1. **CAP-001** `link` / cargo_dep in packages + codegen Cargo.toml.
2. **CAP-002** `HttpHost` port (Rust impl in trampoline; VEIL `@main` calls it).
3. **CAP-003** `register_handlers` emit from gen workspace.

**Exit:** `host.veil` `@main` generates a bin that listens and mounts multi IDE;
trampoline ≤50 lines.

### Sprint 2 — Unblock UI (CAP-005, CAP-006)

1. **CAP-005** Svelte/TS **bundle** pipeline for apps with pages/layouts.
2. **CAP-006** bin layout for runtime workspace if still broken.
3. Point pure-runtime-build at gen `dist/`.

**Exit:** Browser loads **only** generated UI for dashboard/projects.

### Sprint 3 — Domain purity (CAP-004, rewire)

1. **CAP-004** FS/Git ports; storage.veil uses them.
2. Remove `platform.rs` business logic (keep only port impls).
3. Integration tests green.

### Sprint 4 — Finish DoD

1. Agents page, config write (CAP-007), artifacts polish.
2. Delete legacy HTML; CI `make pure-runtime-build` + smoke.
3. Human demo script on clean machine.

---

## 4. What we should **not** invent

| Temptation | Why not |
|------------|---------|
| Rewrite dual-loop IDE in VEIL | Kernel stays Rust (`veil-server`) |
| Put product logic in bootstrap “for speed” | Violates pure-runtime DoD |
| Fake SPA by more hand HTML | Delays CAP-005 forever |
| Require two ports forever | Product path is single origin |

---

## 5. Immediate backlog (add to engine sprint)

| ID | Title | Priority |
|----|--------|----------|
| **CAP-001** | External crate link from VEIL packages | **Done** · P0 |
| **CAP-002** | HttpHost port / host HTTP surface | P0 |
| **CAP-003** | Generated Bus `register_all` | P0 |
| **CAP-005** | Browser-ready UI emit (SPA bundle) | P0 |
| **CAP-004** | FS/Git system ports + DI | P1 |
| **CAP-006** | Runtime multi-crate bin layout | P1 |
| **CAP-007** | Config write API | P2 |

Implementation order: **001 → 002 → 003 → 005 → 004 → 006 → 007**.

---

## 6. Success criteria (restate)

Pure VEIL runtime is **complete** when:

1. Product sources for runtime are only `.veil` / `.layer` / `.stub` under
   `runtime/src/` (plus allowlisted engine crates).
2. Host process is generated from VEIL `@main` (+ optional tiny trampoline).
3. Shell is generated SPA only.
4. All D1 capabilities work through **generated** handlers + ports, not a
   parallel Rust dispatch table.
5. IDE multi-project works same-origin via linked `veil-server`.

Until CAP-001–003 and CAP-005 land, any claim of “pure VEIL front and back” is
premature; platform ops may still be **functionally** good via the current host.
