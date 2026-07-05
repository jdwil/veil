# CORE-002: For Loop / Iteration

**As a** VEIL developer
**I want** to express iteration over collections
**So that** layers can abstract over batch operations and codegen produces Rust `for` loops

## VEIL Syntax

```
for item in items
  call process(item)

for user in call UserRepo.find_all()
  notify UserUpdated{user.id}
```

With index:

```
for i, item in items
  call update_position(item, i)
```

## Generated Rust

```rust
for item in items {
    process(item);
}

for user in deps.user_repo.find_all().await? {
    deps.bus.dispatch(format!("UserUpdated {{ user_id: {:?} }}", user.id)).await?;
}
```

## Acceptance Criteria

- Parser handles `for <binding> in <expr>` followed by indented body
- Optional index variable: `for i, item in <expr>`
- Body contains any valid expressions/statements
- Codegen emits Rust `for` with proper `.await?` on async iterables
- Works at statement position (not as expression — for now)
