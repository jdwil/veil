# CORE-005: Mutable Bindings (let mut)

**As a** VEIL developer  
**I want** to declare mutable variables  
**So that** I can reassign values and the generated Rust uses `let mut`

## VEIL Syntax

```
mut count = 0
count = count + 1
```

## Generated Rust

```rust
let mut count = 0;
count = count + 1;
```

## Acceptance Criteria

- `mut name = expr` produces `let mut name = expr;`
- Subsequent `name = expr` (without mut) produces bare `name = expr;` (reassignment)
- The codegen tracks which variables are mutable
