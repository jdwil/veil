# Pure VEIL runtime (release goal)

Canonical story epic: [`stories/140-pure-veil-runtime.md`](../stories/140-pure-veil-runtime.md).

## Residual non-VEIL (allowed)

| Component | Role |
|-----------|------|
| `veil-parser` / `veil-ir` / `veil-codegen` | Language engine |
| `veil-server` | Dual-loop IDE HTTP kernel (multi-project) |
| `veil-local` | Storage adapters |
| Optional trampoline | ≤50 lines until `@main` host is complete |

Product domain + shell UI must be authored in `.veil` / `.layer` / `.stub`.

## Definition of done (D0–D4)

See story file. Human demo:

```bash
make pure-runtime
# open http://127.0.0.1:8080/
# create project → Open IDE → dual-loop works
```

## Capability matrix

See [`RUNTIME_CAPABILITIES.md`](RUNTIME_CAPABILITIES.md).

## What it takes to finish

Functional multi-host exists; **pure VEIL authorship** is blocked on engine
capabilities:

| CAP | Status |
|-----|--------|
| CAP-001 | **Done** — `link` external crates → generated Cargo.toml |
| CAP-002 | **Done** — `veil_server::ProductHost` + host.veil `@main` |
| CAP-003 | **Done** — `veil_shared::register_all` / HANDLER_NAMES |
| CAP-005 | **Done** — SPA `dist/` emit for UI packages |
| CAP-004 | **Done** — FileSystem / GitRepo local adapters |
| CAP-006 | **Done** — ProductHost `veil_bin` when linking veil-server |
| CAP-007 | **Done** — `PATCH /api/config` allowlisted keys |

Full plan: [`stories/141-pure-runtime-capability-gaps.md`](../stories/141-pure-runtime-capability-gaps.md).

**PVR-011:** Bus List/Create/Read/Write/ListFiles/Branches/Log call generated
`storage::application` with CAP-004 `local_ports`. Compile/deploy remain host
helpers. Escape hatch: `VEIL_PLATFORM_LEGACY=1`. Shell: generated SPA under
`static/dist`.

```bash
make pure-runtime-build   # gen runtime.veil + SPA + trampoline
make pure-runtime-smoke   # curl health / projects / config / SPA asset
make pure-runtime         # build + serve :8080
```
