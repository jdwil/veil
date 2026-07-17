# UI extension mount slots (EXT-12)

| Field | Value |
|-------|-------|
| **Service** | `MountUiExtension` (VEIL) / bus `MountUiExtension` |
| **ABI** | `mount(extension_id, version, slot, props) → { mount_id, asset_uri }` |

## Slot registry (products)

| Slot | Product | Purpose |
|------|---------|---------|
| `wear_test.rules.panel` | wear_test | Extra panel on Rules page |
| `wear_test.initiative.wizard.step` | wear_test | Custom wizard step |
| `application.signal.inspector` | application | Complex signal UI |

Products declare which slots they mount; runtime resolves `asset_uri` from `ExtensionArtifactStore` (local path or `s3://…`).

## Dual-loop

`FileExtensionArtifactStore` writes under `VEIL_EXTENSIONS_DIR/artifacts/…`.  
`MountUiExtension` returns `local://extensions/{slot}/{id}@{version}` for host shell to load.

## Complex Signal

`ComplexSignalSpec.extension: Opt<ExtensionRef>` — same pin model as Reaction. Evaluate path should call `ExtensionInvokePort` / `InvokeExtension` when set (bridge: `veil_source_ref` still allowed).
