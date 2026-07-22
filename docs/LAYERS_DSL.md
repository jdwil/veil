# Shipping team DSL layers

Product-family languages (wear test, loyalty, …) live as `.layer` files.
Client products are **packages** that `use` those layers plus platform packages
(`dlx_core`, providers, signals). Soft knobs stay data; control flow stays in packages.

## Project layout

```
layers/
  wear_test.layer      # family vocabulary
  loyalty.layer
examples/
  wear_test.veil       # reference package (must stay green)
  brooks_wear_….veil   # client programs (often private repos)
```

`veil serve examples` also loads workspace `layers/*.layer` into the file list.

## Composition

- Prefer **small layers** that `use` shared foundations (`ddd`, platform).
- Version with `pkg name vN` and changelog breaking construct renames.
- Keep a **reference package** that exercises the layer; run `veil check` on it in CI.

## Designer loop (IDE)

1. Open a `.layer` in the file switcher (📐).
2. Edit source / structured create construct / check.
3. Save → registry hot-reloads packages that `use` the layer (no serve restart).
4. Switch to a reference package to verify palette + presentation.

## CLI

```bash
veil check layers/wear_test.layer
veil check examples/wear_test.veil -t rust
veil serve examples -p 3001   # packages + layers
```

Scaffold:

```bash
curl -X POST http://127.0.0.1:3001/api/layer/scaffold \
  -H 'Content-Type: application/json' \
  -d '{"name":"my_dsl","desc":"My team language"}'
```

## Impact

`GET /api/layer/dependents?layer=ddd` lists packages in the serve set that `use` that layer.

## Policy roles (INV-001)

Domain and DI vocabulary live in layers via **annotation roles** and **policy
blocks** (`bus_policy`, `auth_policy`, `http_name_policy`, …). The engine never
hard-codes names like `"route"`, `"dep"`, or `"Handle"`.

See [`POLICY_ROLES.md`](./POLICY_ROLES.md) for the full catalog and proposed
`veil.toml` / policy-pack overrides.
