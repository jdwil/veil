# CORE-003: While Loop

**As a** VEIL developer
**I want** to express conditional loops
**So that** layers can abstract over retry logic, polling, etc.

## VEIL Syntax

```
while status != Done
  call poll_status()
  wait(1000)
```

## Generated Rust

```rust
while status != Done {
    poll_status();
    wait(1000);
}
```

## Acceptance Criteria

- Parser handles `while <condition>` followed by indented body
- Codegen emits Rust `while` with braces
- Works at statement position
