# Bootstrap — thin trampoline (CAP-002 / PVR-010)

Process entry for **veil-runtime**. Product HTTP surface is
`veil_server::ProductHost` (IDE multi + SPA + config). Bus dispatch for
storage/tools uses CAP-004 ports in `platform.rs` until generated crates fully
own handler bodies.

## Prefer

```bash
make pure-runtime          # build + serve product host
make pure-runtime-smoke    # CI-friendly curl smoke
```

VEIL host package (ProductHost bin, no bus platform yet):

```bash
veil gen runtime/src/host.veil -t rust -o runtime/generated-host
cd runtime/generated-host && cargo run -p veil_bin
```

App harness demos (not the product runtime):

```bash
scripts/run_local_example.sh
```

## Layout

| Path | Role |
|------|------|
| `src/main.rs` | Trampoline: bus routes + `ProductHost::listen` |
| `src/platform.rs` | Live handlers + `FileSystem`/`GitRepo` + `register_all` |
| `static/dist/` | **Primary** generated SPA (`make pure-runtime-build`) |
| `static/ide.html` | IDE iframe embed shell |
| `static/legacy/` | Quarantined notes for old hand HTML |

Do not grow product UI or domain logic here — author `.veil` under `runtime/src/`.
