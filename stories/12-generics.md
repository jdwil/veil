# CORE-012: Generics

**As a** VEIL developer
**I want** to declare type parameters on constructs
**So that** I can write reusable types and the codegen produces proper Rust generics

## VEIL Syntax

```
struct Pair<A, B>
  first: A
  second: B

trait Repository<T>
  find(id: UUID) -> Res!<T>
  save(item: T) -> Res!
```

## Generated Rust

```rust
pub struct Pair<A, B> {
    pub first: A,
    pub second: B,
}

#[async_trait]
pub trait Repository<T> {
    async fn find(&self, id: Uuid) -> Result<T, DomainError>;
    async fn save(&self, item: T) -> Result<(), DomainError>;
}
```

## Acceptance Criteria

- Constructs can declare type parameters: `struct Name<A, B>`
- Type parameters are passed through to generated Rust code
- Type parameters can be used in field types and method signatures
