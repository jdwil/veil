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

| Feature | Rust | TypeScript | Swift (spike) | Kotlin (spike) |
|---------|------|------------|---------------|----------------|
| RangeExpr | yes | gated | no | no |
| Closures | yes | partial | no | no |
| MatchExpr | yes | yes | no | no |
| AwaitExpr | yes | yes | no | no |
| TryOperator | yes | yes | no | no |
| FnBodyLowering | yes | yes | **no** (sig-only) | **no** (sig-only) |
| EmptyAdapterBody | warn/escape | — | no | no |
| EmptyUiTemplate | — | allowed shell | no | no |
| ImplBlocks | yes | n/a | no | no |
| RawBlocks | escape debt | escape debt | no | no |

**PAR-015:** Spikes do **not** claim expression features. Non-empty function
bodies → `unsupported_fn_body_lowering` on `-t swift|kotlin`. Struct/enum
signatures still emit; gen may still stub bodies, but **check fails closed**.

Exact sets live in code — this table is orientation only.

### CLI targets

```bash
veil gen pkg.veil -t rust|typescript|swift|kotlin -o OUT
veil check pkg.veil -t swift   # fails closed on unsupported features
```
