# ACR Bootstrap v0 — Design Decisions

## Overview

The Algorithmic Cognition Runtime (ACR) is a cognitive architecture whose primary learned artifact is a growing library of validated executable algorithms. External LLMs act as teachers that grow the library via MCP tools. The runtime operates independently using only promoted algorithms.

## Architecture

```
┌─────────────────────────────┐
│  Teacher LLMs (MCP tools)   │
│  propose / test / edit      │
└───────────┬─────────────────┘
            │ HTTP JSON API (:3100)
┌───────────▼─────────────────┐
│  acr-mcp                    │
│  8 tool endpoints           │
└───────────┬─────────────────┘
            │
┌───────────▼─────────────────┐
│  acr-eval                   │     ┌──────────────┐
│  harness, scoring, tasks    │────▶│  acr-core    │
└───────────┬─────────────────┘     │  IR, executor│
            │                       │  trace, value│
┌───────────▼─────────────────┐     └──────────────┘
│  acr-library                │            ▲
│  versioned store (FsStore)  │            │
└───────────┬─────────────────┘            │
            │ promotes                     │
┌───────────▼─────────────────┐            │
│  acr-runtime                │────────────┘
│  selector → execute → state │
└─────────────────────────────┘
```

## Key Design Decisions

### 1. Algorithm IR: Custom AST (not WASM, not Rust subset)

**Decision:** A purpose-built expression language with typed values, control flow, and built-in functions.

**Rationale:**
- Full Rust is too complex to sandbox safely without process isolation
- WASM adds compilation complexity and is overkill for v0
- A custom AST is directly serializable (serde JSON), diffable, and inspectable
- The IR is restricted enough to guarantee termination (via step limits) without analyzing halting properties
- Teacher LLMs can easily construct/modify the JSON AST

**IR constructs:**
- Statements: Let, Assign, If, While, For, Return, Expr, Assert
- Expressions: Literal, Var, BinOp, UnaryOp, Call, Index, FieldAccess, ListLiteral, MapLiteral, Lambda
- Types: Null, Bool, Int, Float, Str, List, Map
- 19 built-in functions (len, push, pop, head, tail, contains, concat, slice, sort, reverse, map_get, map_set, map_keys, to_str, to_int, split, join, type_of, print)

### 2. Sandbox Strategy: Tree-Walking Interpreter with Limits

**Decision:** Interpreted execution with step counting, stack depth limits, and list size caps.

**Rationale:**
- No process isolation needed because the IR has no I/O, no system calls, no FFI
- Step limit (default 10,000) guarantees bounded execution time
- Stack depth limit (default 100) prevents infinite recursion
- List size limit (default 10,000) prevents memory exhaustion
- Deterministic: same input always produces same output (no randomness in v0)

**Trade-offs:**
- Slower than compiled execution (acceptable for v0)
- No parallelism within algorithm execution
- Lambda/closures stubbed (can be added later)

### 3. First Curriculum Domain: List/String Manipulation

**Decision:** 5 tasks in list and string manipulation.

**Rationale:**
- Concrete, testable, unambiguous expected outputs
- Progressively harder (Easy → Medium)
- Exercises core IR features (loops, conditionals, built-in functions)
- Good signal for evaluating algorithm quality (pass/fail per case)

**Tasks:**
1. list-reverse (Easy) — reverse without built-in
2. filter-even (Easy) — filter integers by evenness
3. is-palindrome (Easy) — check if string reads same forwards/backwards
4. list-flatten (Medium) — flatten nested lists
5. list-dedup (Medium) — remove duplicates preserving order

### 4. Metadata Store: Local Filesystem (JSON)

**Decision:** `FsStore` implementation using tokio::fs with JSON serialization.

**Rationale:**
- Zero infrastructure for development
- Same trait (`AlgorithmStore`) can be implemented for DynamoDB/S3 later
- Directory layout is inspectable with standard tools
- Sufficient for single-machine development and testing

**Layout:**
```
{base}/algorithms/{uuid}/entry.json   — AlgorithmEntry metadata
{base}/algorithms/{uuid}/v{N}.json    — Algorithm at version N
{base}/traces/{trace_id}.json         — ExecutionTrace
```

### 5. MCP Interface: HTTP JSON API (not stdio MCP)

**Decision:** Axum HTTP server with POST endpoints at `/tools/*`.

**Rationale:**
- Easier to test and debug than stdio JSON-RPC
- Works with any HTTP-capable LLM tool integration
- Can be wrapped in stdio MCP adapter later if needed
- Stateless handlers with shared `Arc<AppState>`

**Endpoints:**
| Tool | Path | Purpose |
|------|------|---------|
| list_algorithms | POST /tools/list_algorithms | Query library with filters |
| create_candidate | POST /tools/create_candidate | Create new algorithm |
| update_candidate | POST /tools/update_candidate | New version of existing |
| run_evaluation | POST /tools/run_evaluation | Test against a task |
| get_trace | POST /tools/get_trace | Inspect execution details |
| promote | POST /tools/promote | Move to promoted library |
| list_tasks | POST /tools/list_tasks | Available evaluation tasks |
| get_library_status | POST /tools/get_library_status | Counts and status |

### 6. Runtime Selection: Domain + Tag Match with Score Ranking

**Decision:** Crude selector that filters by domain and promotion status, ranks by tag overlap and historical scores.

**Rationale:**
- Simple enough to implement and understand
- Sufficient for single-domain v0
- Clear upgrade path (embedding-based similarity, capability matching)
- "First success wins" execution strategy is predictable

### 7. Versioning and Provenance

**Decision:** Every algorithm has explicit version numbers, and every version records its provenance (generated, mutated, composed, manual).

**Rationale:**
- Required by spec for evaluation history
- Enables diffing between versions
- Supports the learning loop narrative (which prompt/mutation produced improvement)
- Immutable versions prevent accidental regression

## What's Stubbed / Deferred

| Feature | Status | Path to implement |
|---------|--------|-------------------|
| Lambda/closure execution | Stubbed (returns error) | Add closure Value type + environment capture |
| Hierarchical dependencies | Declared in metadata, not resolved at runtime | Add dependency graph loader |
| Tiny neural surface | Not implemented | Trait + small CPU model (candle/llama.cpp) |
| DynamoDB/S3 backend | Interface ready | Implement AlgorithmStore for AWS SDK |
| Stdio MCP transport | HTTP only | Add transport adapter |
| Algorithm composition | Provenance only | Allow Call to invoke other library algorithms |
| Parallel learning trials | Sequential only | Spawn on ECS/Spot workers |
| Hard pruning at load time | All promoted loaded | Add capability/relevance pre-filter |

## Running

```bash
# Run tests
cd acr && cargo test --workspace

# Start MCP server
cd acr && cargo run -p acr-mcp
# Listens on :3100, stores in ./acr-data/

# Environment variables
ACR_LIBRARY_PATH=/path/to/data   # default: ./acr-data
ACR_PORT=3100                     # default: 3100
```

## Success Criteria Met

1. ✅ A teacher LLM can, via tools only, propose an algorithm, run it on tests, improve it, and promote a version
2. ✅ Runtime path executes without the teacher in the loop
3. ✅ Clear separation between candidate workspace and promoted library (PromotionStatus enum)
4. ✅ Integration test demonstrates the complete loop end-to-end
