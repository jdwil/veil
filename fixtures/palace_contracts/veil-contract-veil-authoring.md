# veil-contract-veil-authoring

**Type:** Concept  
**Summary:** Product VEIL dual-loop pitfalls — multi-tenant identity, execute fail-closed, unscoped lists, svc vs handler, veil test.  
**Links:** veil-contract-inv001-harness, veil-harness-devloop, veil-ddd-layer, veil-di-layer, veil-project-index

## Multi-tenant identity

Harness API key is one global secret. Body/query `tenant_id` alone is not multi-tenant security.

**Do:** scope tenant-owned ops; fail closed on unscoped integration-style `list()`.  
**Don't:** trust client `tenant_id` under a shared key as isolation.

## Secrets in VEIL

**Do:** `@secret` on tokens/passwords; persist full JSON in adapters; `veil test` for round-trips.  
**Don't:** invent skip_serializing on domain types for API safety.

## HTTP execute

**Do:** fail closed unsupported auth; non-2xx → External; empty body → `{}`; client timeouts via stub.  
**Don't:** map upstream failures to Validation (400).

## svc vs handler

**Do:** logic on `svc`; HTTP on `handler` + `@route`; names match after Handle strip.  
**Don't:** duplicate full bodies in both.

## Tests

```bash
veil test main.veil
```

VEIL `it`/`given`/`then` are real tests. "No cargo tests in generated/" ≠ "no VEIL tests."
