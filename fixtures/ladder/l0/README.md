# L0 — Hello svc + memory repo

**Skills:** `ctx`, `port`, handler `svc`, `ret`, memory `impl`.

## DO

- One context, one aggregate, one port, list + create handlers
- Memory adapter with `ret []` / `ret Ok` / `ret null`
- Prefer `@route("METHOD /path")` on every public handler

## DON'T

- Multi-package or external SDKs (see L2 / L3)
- `.unwrap()` / invent REST paths without `@route`
- Empty infrastructure group (harness needs an adapter)

## Verify

```bash
veil check fixtures/ladder/l0/hello.veil
veil gen fixtures/ladder/l0/hello.veil -o /tmp/ladder-l0 -t rust
cd /tmp/ladder-l0 && cargo check -p veil_bin
```

Or: `make fixture-ladder-l0`
