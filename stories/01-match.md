# CORE-001: Match / Pattern Matching

**As a** VEIL developer
**I want** to express pattern matching in VEIL source
**So that** layers can abstract over branching logic and codegen produces Rust `match` arms

## VEIL Syntax

```
match status
  Pending -> handle_pending()
  Verified -> handle_verified(code)
  _ -> handle_default()
```

With expressions:

```
result = match response.status()
  200 -> parse_body(response)
  404 -> ret Err("not found")
  _ -> ret Err("unexpected")
```

With destructuring:

```
match event
  CustomerCreated{id, email} -> notify(id, email)
  CustomerVerified{id} -> activate(id)
```

## Generated Rust

```rust
match status {
    CustomerStatus::Pending => handle_pending(),
    CustomerStatus::Verified => handle_verified(code),
    _ => handle_default(),
}
```

## Acceptance Criteria

- Parser handles `match <expr>` followed by indented arms
- Each arm: `<pattern> -> <expr>` or `<pattern> -> <block>`
- Patterns: identifiers, literals, `_` wildcard, struct destructuring
- Codegen emits proper Rust `match` with `=>` arms
- Works as both statement and expression (assignable)
- Enum variants auto-qualified with type name when context is known
