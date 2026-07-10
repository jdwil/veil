# Package / expose codegen by target (GEN-008)

How `veil gen` treats `VeilFile::Package` vs `Solution` per `CodegenTarget`.

| Target | Package behavior | `expose` block |
|--------|------------------|----------------|
| **rust** | Convert package → solution-shaped IR; full crate workspace | Not used for API clients; exports via visibility |
| **typescript** | If package has `expose` → typed API client; else module sources | **Primary** consumer — generates client types |
| **swift** | Convert package → solution; struct/enum/fn sources (PAR-011) | No-op (document only); warn if only expose present |
| **kotlin** | Same as swift (PAR-012) | No-op |

## CLI

```bash
veil gen pkg.veil -t rust -o OUT
veil gen pkg.veil -t typescript -o OUT   # API client when expose present
veil gen pkg.veil -t swift|kotlin -o OUT
veil check pkg.veil -t swift             # capability matrix
```

## Library packages (PAR-008)

`examples/pure_lib.veil` has **no** Bus and **no** expose — gen works on all
targets for types + free functions. Add `expose` only when TS clients need a
stable boundary contract.

## Honesty

Targets that ignore `expose` must not claim a client was generated. Prefer
`veil check -t <target>` for capability failures over silent empty clients.
