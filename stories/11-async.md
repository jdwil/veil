# CORE-011: Async/Await

**As a** VEIL developer
**I want** to express async operations and awaiting
**So that** I can control concurrency and the codegen knows where to place .await

## VEIL Syntax

```
result = await call fetch_data(url)
```

Or implicit (all port calls are already async):
```
data = call Repo.find(id)  # already generates .await?
```

Explicit await for non-port calls:
```
response = await http.get(url)
```

## Generated Rust

```rust
let result = fetch_data(url).await?;
let response = http.get(url).await;
```

## Design Decision

In VEIL, async is mostly invisible — port/trait calls always generate `.await?`.
The `await` keyword is for explicitly marking non-port calls as async.
This is primarily for use with stub crate methods.
