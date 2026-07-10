# Backend capability matrix (PAR-002 / CHK-005)

Machine-readable source: `crates/veil-codegen/src/capabilities.rs`  
Enum: `veil_codegen::capabilities::Feature`

Wired into `veil check -t <target>` (**primary target only** by default).

Cross-target “debt” warnings (`capability_debt` for *other* languages) are
**opt-in**: `veil check -t rust --target-debt` or `?target_debt=true` on the API.
Prefer switching `-t ts` (etc.) when you want real target-gated errors.

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

**PAR-011/012:** Swift/Kotlin claim `FnBodyLowering` for core exprs (literals,
fields, binary ops, `ret`, `if`, struct lit, simple match/try). Advanced
features (range, closures, …) still fail closed.

**PAR-015:** Signature-only honesty until body lowering landed; matrix above
is current.

Exact sets live in code — this table is orientation only.

### CLI targets

```bash
veil gen pkg.veil -t rust|typescript|swift|kotlin -o OUT
veil check pkg.veil -t swift   # fails closed on unsupported features
```
