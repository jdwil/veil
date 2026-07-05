# CORE-004: Closures / Lambdas

**As a** VEIL developer
**I want** to express closures inline
**So that** I can use iterators, callbacks, and functional patterns

## VEIL Syntax

```
doubled = items.map(|x| x * 2)
filtered = users.filter(|u| u.active == true)
result = items.fold(0, |acc, x| acc + x)
```

Multi-line:
```
handler = |event|
  call process(event)
  call notify(event.id)
```

## Generated Rust

```rust
let doubled = items.map(|x| x * 2);
let filtered = users.filter(|u| u.active == true);
let result = items.fold(0, |acc, x| acc + x);

let handler = |event| {
    process(event);
    notify(event.id);
};
```

## Acceptance Criteria

- Parser handles `|params| expr` for single-expression closures
- Parser handles `|params|\n  body` for multi-line closures
- Closures work as arguments to method calls
- Codegen emits Rust closure syntax with proper `{}` for multi-line
