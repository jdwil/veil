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
| `src/platform.rs` | Bus dispatch + compile/deploy + `register_all` |
| `src/local_ports.rs` | CAP-004 local MetadataStore/ObjectStorage for generated storage |
| `../generated/crates/storage` | Generated domain services (PVR-011) |
| `static/dist/` | **Primary** generated SPA (`make pure-runtime-build`) |
| `static/viewer/` | Dual-loop IDE (built `veil-viewer`, `/viewer`) |
| `static/legacy/` | Quarantined hand HTML only (not served as primary shell) |

Do not grow product UI or domain logic here — author `.veil` under `runtime/src/`.
