# Backend capability matrix (PAR-002 / CHK-005)

Machine-readable source: `crates/veil-codegen/src/capabilities.rs`  
Enum: `veil_codegen::capabilities::Feature`

Wired into `veil check -t <target>` and multi-target debt warnings.

## How to extend (Swift / Kotlin / …)

1. Add `CodegenTarget` variant if needed.  
2. Implement `supported_features(target) -> HashSet<Feature>`.  
3. Unsupported features used in a package → **error** on that target.  
4. Document in this file.

## Snapshot (honest defaults)

| Feature | Rust | TypeScript |
|---------|------|------------|
| RangeExpr | yes | gated |
| Closures | yes | partial |
| MatchExpr | yes | yes |
| AwaitExpr | yes | yes |
| TryOperator | yes | yes |
| EmptyAdapterBody | warn/escape | — |
| EmptyUiTemplate | — | allowed shell |
| ImplBlocks | yes | n/a |
| RawBlocks | escape debt | escape debt |

Exact sets live in code — this table is orientation only.
