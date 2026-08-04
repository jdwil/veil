# VEIL Conversion Spec — Meta-Layer Execution & Deployment Contexts

## Overview

This spec guides an agent to integrate hand-written VEIL drafts into the runtime,
compile them, and verify semantic equivalence against the reference Rust implementation
in `/home/jd/dev/jd/veil-runtime/`.

## Draft Files (ready to integrate)

- `/home/jd/dev/jd/veil/runtime/src/meta_execution.veil.draft` — MetaExecution context (259 lines)
- `/home/jd/dev/jd/veil/runtime/src/deploy.veil.draft` — Deploy context (496 lines)

## Target File

- `/home/jd/dev/jd/veil/runtime/src/runtime.veil` — append both contexts to the end

## Execution Steps

### Step 1: Read the existing runtime.veil ending

Read the last 20 lines of `runtime.veil` to find where to append. The file should
end after the Extensions context's `group infrastructure` section closes.

### Step 2: Append MetaExecution context

Append the contents of `meta_execution.veil.draft` to the end of `runtime.veil`.
Ensure proper indentation (2 spaces for ctx-level content within the pkg block).

### Step 3: Append Deploy context

Append the contents of `deploy.veil.draft` to the end of `runtime.veil`.

### Step 4: Verify `use` statements

The MetaExecution and Deploy contexts may need additional `use` statements at
the package level (top of the file). Check if these are already present:
- `use aws_sdk_lambda` — already exists as stub
- `use aws_sdk_sqs` — already exists as stub

If any `use` is missing, add it to the existing `use` block at the top of the file.

### Step 5: Compile

```bash
cd /home/jd/dev/jd/veil && make pure-runtime-build
```

If compilation fails:
1. Read the error message
2. Fix the VEIL source (common issues: missing fields, wrong type names, indentation)
3. Retry compilation
4. Repeat until it passes

### Step 6: Verify generated output

After successful compilation, check:
```bash
ls runtime/generated/crates/
```

New crates should appear for the MetaExecution and Deploy contexts. Compare their
generated Rust against the reference:

- Generated meta types vs `/home/jd/dev/jd/veil-runtime/crates/veil-runtime-core/src/meta.rs`
- Generated deploy types vs `/home/jd/dev/jd/veil-runtime/crates/veil-runtime-deploy/src/state.rs`
- Generated tools vs `/home/jd/dev/jd/veil-runtime/crates/veil-runtime-deploy/src/tools.rs`

Semantic equivalence means: same type names, same fields, same method signatures.
The generated code may have different formatting or slightly different patterns
(e.g., different error handling macros) — that's expected.

### Step 7: Clean up drafts

Once successfully compiled and verified, delete the draft files:
```bash
rm runtime/src/meta_execution.veil.draft
rm runtime/src/deploy.veil.draft
```

## Critical Rules

1. All domain logic MUST be in `.veil` — not hand-written Rust
2. Follow EXISTING patterns in `runtime.veil` exactly
3. The `runtime.veil` file is ONE file with multiple `ctx` blocks inside a single `pkg`
4. Indentation: 2 spaces per level (pkg > ctx > group > type/svc/tool)
5. Do NOT modify the hand-written Rust in `/home/jd/dev/jd/veil-runtime/` — it's reference only
6. If the VEIL compiler rejects syntax, check similar constructs in the existing
   Storage/Tools/Daemon/Exec/Extensions contexts for the correct pattern

## Reference Patterns

### Port method (effectful = `!`)
```veil
port MetaLayerExecutor
  execute!(request: ExecutionRequest) -> ExecutionResult
```

### Service with deps
```veil
svc ExecuteMetaFunction
  -> ExecutionResult
  @dep(objects: ObjectStorage)
  input
    function_id: MetaFunctionId
  step execute
    ...
```

### Tool with summary
```veil
tool DeployTool
  -> Json
  @desc("Deploy a unit...")
  input
    unit_name: Str
  step execute
    result = invoke DeployUnit{...}
    ret {version: result.version, summary: f"Deployed..."}
```

### Enum with variants
```veil
enum ActionRisk
  Low
  Medium
  High
```

### Val (value object)
```veil
val DeploymentState
  project: Str
  version: Int
  status: DeploymentStatus
```

## Troubleshooting

### Common VEIL compiler errors:

- **"unknown type X"** — Check spelling, ensure the type is defined BEFORE it's used in the same context
- **"expected indent"** — VEIL is indent-sensitive; check 2-space alignment
- **"port method must end with !"** — Effectful port methods need `!` suffix
- **"cannot invoke across contexts"** — Services can only invoke within their own context; use Bus for cross-context
- **"field not found"** — Val/Enum fields are positional in constructors; check order matches definition

### If `make pure-runtime-build` fails on unrelated code:

The existing runtime.veil may have pre-existing issues. Run `make pure-runtime-build`
BEFORE making changes to establish a baseline. If it already fails, note which errors
are pre-existing vs introduced by the new contexts.
