# VEIL

## Session start (non-negotiable)

1. **Mind Palace first** for any project work: `mind-palace__wiki_search` → `mind-palace__wiki_read` **before** monorepo `grep` / `read_file` / exploratory shell.
2. Full policy: [`.grok/rules/mind-palace.md`](.grok/rules/mind-palace.md)
3. Local runtime AWS (jd@dashlx.com): palace `local-veil-runtime-dev-jd` and [`runtime/docs/SOURCE_STORE.md`](runtime/docs/SOURCE_STORE.md) — `AWS_PROFILE=dashlx_dev`, `VEIL_DDB_TABLE=veil-runtime-dev`, `BUCKET=veil-runtime-dev`.

## Agent control model

The runtime agent must be able to drive the full UX (navigate, IDE, SDLC, deploy, compile) via tools — not hard-coded Svelte shortcuts. See palace: `runtime-omnipresent-agent-design`, `decision-unified-agent-acp-tunnel`.

## Single ProductHost (no dual veil serve)

Product UX is **one process**: `ProductHost` (bootstrap / pure-runtime) links
`crates/veil-server` (IDE kernel) and serves IDE UI from `runtime/ide-ui` at
`/viewer`. Do **not** run a separate `veil serve --multi` just for the dashboard
agent. Details: [`runtime/docs/ADR_SINGLE_PRODUCT_HOST.md`](runtime/docs/ADR_SINGLE_PRODUCT_HOST.md).

### Local stack (agents manage this)

```bash
runtime/scripts/dev-stack.sh restart   # backend :8080 + UI :5180
runtime/scripts/dev-stack.sh status
runtime/scripts/dev-stack.sh smoke
# After backend code changes: rebuild then
cargo build --release --manifest-path runtime/bootstrap/Cargo.toml
runtime/scripts/dev-stack.sh backend
# UI-only reload:
runtime/scripts/dev-stack.sh ui
```

Logs: `/tmp/veil-product-host.log`, `/tmp/veil-ui.log`.
Env: `AWS_PROFILE=dashlx_dev`, `VEIL_DDB_TABLE=veil-runtime-dev`, `BUCKET=veil-runtime-dev`.

## VEIL authorship (non-negotiable)

Product logic lives in `.veil` / `.layer` / `.stub`. **Never hand-edit generated outputs.**

| Do | Do not |
|----|--------|
| Edit `runtime/src/runtime-ui.veil` for shell/product UI | Edit `runtime/ui/src/lib/components/*.svelte` product views |
| `veil gen … -t typescript` then copy regenerated files | “Quick fix” in Svelte and leave VEIL stale |
| Fix codegen in `crates/veil-codegen` | Patch `generated/` or `runtime/generated/` |

**Hard rule:** [`.grok/rules/veil-no-hand-edit-generated-ui.md`](.grok/rules/veil-no-hand-edit-generated-ui.md)  
Palace: `veil-editing-patterns`, `veil-contract-no-hand-edit-generated`.  
Exceptions: `runtime/ui/src/lib/ide/**`, `runtime/ui/src/lib/agent/**` (not VEIL-generated).
