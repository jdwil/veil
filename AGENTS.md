# VEIL

## Session start (non-negotiable)

1. **Mind Palace first** for any project work: `mind-palace__wiki_search` → `mind-palace__wiki_read` **before** monorepo `grep` / `read_file` / exploratory shell.
2. Full policy: [`.grok/rules/mind-palace.md`](.grok/rules/mind-palace.md)
3. Local AWS (jd@dashlx.com): palace `local-veil-runtime-dev-jd` and [`docs/SOURCE_STORE.md`](docs/SOURCE_STORE.md) — `AWS_PROFILE=dashlx_dev`, `VEIL_DDB_TABLE=veil-runtime-dev`, `BUCKET=veil-runtime-dev`.

## Agent control model

The runtime agent must be able to drive the full UX (navigate, IDE, SDLC, deploy, compile) via tools — not hard-coded Svelte shortcuts. See palace: `runtime-omnipresent-agent-design`, `decision-unified-agent-acp-tunnel`.

## Single ProductHost (no dual veil serve)

Product UX is **one process**: `crates/veil-runtime` (ProductHost) links
`crates/veil-server` (IDE kernel) and serves the shell from `ui/` (Vite :5180)
plus optional `/viewer` from `ide-ui`. Do **not** run a separate
`veil serve --multi` just for the dashboard agent.
Details: [`docs/ADR_SINGLE_PRODUCT_HOST.md`](docs/ADR_SINGLE_PRODUCT_HOST.md).

### Local stack (agents manage this)

```bash
scripts/dev-stack.sh restart   # backend :8080 + UI :5180
scripts/dev-stack.sh status
scripts/dev-stack.sh smoke
# After backend code changes: rebuild then
cargo build --release -p veil-runtime
scripts/dev-stack.sh backend
# UI-only reload:
scripts/dev-stack.sh ui
```

Logs: `/tmp/veil-product-host.log`, `/tmp/veil-ui.log`.
Env: `AWS_PROFILE=dashlx_dev`, `VEIL_DDB_TABLE=veil-runtime-dev`, `BUCKET=veil-runtime-dev`.

## Where code lives

| Layer | Location | Language |
|----|----|----|
| Product host (repos, PRs, deploy, shell static) | `crates/veil-runtime` | Rust |
| IDE kernel, MCP, sessions, agent tools | `crates/veil-server` | Rust |
| Shell + native IDE + agent dock | `ui/` | Svelte |
| Customer products | `.veil` / `.layer` / `.stub` | VEIL |

The host is **not** authored in VEIL (palace: `decision-runtime-not-veil-dogfood`).
Customer products **are**. Never hand-edit VEIL-generated customer outputs.

**Hard rule for customer VEIL:** [`.grok/rules/veil-no-hand-edit-generated-ui.md`](.grok/rules/veil-no-hand-edit-generated-ui.md)
