# VEIL Canonical Serialization Format

**SER-003.** The serializer produces a **canonical** form of VEIL source from the
AST. Agents and the IDE editor rely on this so re-save does not create noisy
diffs.

## Rules

| Rule | Canonical choice |
|------|------------------|
| Indentation | **2 spaces** per block level |
| Package keyword | **`pkg`** (`sol` is deprecated and is rewritten to `pkg` on emit) |
| Export / public | **`+`** prefix (`export ` is rewritten to `+`) |
| Calls | **Bare** `Target.method(args)` or `name(args)` — never the `call` keyword |
| Statement sugar | Preserve layer sugar (`dispatch`, `guard`, …) when `CallExpr.sugar` is set |
| Item order | **AST / source order** — never sorted alphabetically |
| Annotations | Source order; each on its own line immediately above the annotated item/field |
| Field defaults | `name: Type = expr` when `default_expr` is set |
| Typed assigns | `name: Type = expr` and `mut name: Type = expr` preserve the type annotation |
| Enum variant lits | `Enum.Variant{fields}` (dot form; never `Enum::Variant` — no `::` token) |
| Blank lines | Exactly **one** blank line between emitted top-level items |
| Layer-provided | Injected declare items are **omitted**; no blank left in their place |
| Trailing newline | File ends with a **single** `\n` (no trailing blank lines) |

## Idempotence

For any package that parses cleanly:

```
parse(source) → AST₁ → emit → text₁ → parse → AST₂ → emit → text₂
```

`text₁` must equal `text₂`. CLI check:

```bash
veil emit app.veil > /tmp/a.veil
veil emit /tmp/a.veil > /tmp/b.veil
diff /tmp/a.veil /tmp/b.veil   # empty
```

## What is not canonicalized (yet)

- Comments (not in AST; dropped on re-emit)
- Original spacing within expressions (operators get single spaces)
- Span-preserving partial file rewrite (full re-emit only)

## Round-trip suite (SER-004)

```bash
make test-roundtrip
# or
cargo test -p veil-parser --test roundtrip_suite
```

Covers every `examples/*.veil` and `runtime/src/*.veil`. Green fixtures must be
emit-idempotent. Currently allowlisted as unparseable (svelte string bodies):
`customer_portal.veil`, `runtime-ui.veil`.

## Edit identity (SER-005)

Structured edits (`EditOp`) are keyed by **AST span start** (`node.span.start`),
not ephemeral IR node ids. After a successful save the server returns a fresh
IR; subsequent edits must use the new spans.

`set_body` sends VEIL expression source strings; the server parses each line
with `veil_parser::parse_expr_str` into real `Expr` nodes. Invalid body text
returns `400` and **does not** write the file.

## Related

- SER-001: field annotations and defaults
- SER-002: full control-flow bodies (`emit_expr`)
- SER-003: canonical format + churn control
- SER-004: fixture round-trip suite
- SER-005: structured edit ops + real body parse
- Implementation: `crates/veil-ir/src/serialize.rs`, `crates/veil-ir/src/edit.rs`
- Suite: `crates/veil-parser/tests/roundtrip_suite.rs`
