# Mind Palace + VEIL IDE agents

Mind Palace is the durable wiki-style knowledge base for VEIL agents. It is a
**separate project** ([jdwil/mind-palace](https://github.com/jdwil/mind-palace)),
pulled by Cargo as a git dependency of `veil-server` — not monorepo source.

IDE agent dock / ACP / dual-loop overview: [IDE_AGENT_PLATFORM.md](./IDE_AGENT_PLATFORM.md).

```toml
# crates/veil-server/Cargo.toml
mind-palace = { git = "https://github.com/jdwil/mind-palace" }
mind-palace-rig = { git = "https://github.com/jdwil/mind-palace" }
```

Optional local path override while hacking:

```toml
mind-palace = { path = "../../../mind-palace/crates/mind-palace" }
mind-palace-rig = { path = "../../../mind-palace/crates/mind-palace-rig" }
```

## Enable (local IDE → dashlx_dev AWS)

```bash
export AWS_PROFILE=dashlx_dev   # aws sso login --profile dashlx_dev
export MIND_PALACE=1

# Region MUST match where SAM deployed the stack.
# dashlx_dev (account 086261225885): us-west-2  ← NOT us-east-1
export MIND_PALACE_REGION=us-west-2

# Bucket names only — no s3:// prefix (engine strips s3:// if present)
export MIND_PALACE_S3_BUCKET=mind-palace-pages-dev-086261225885
export MIND_PALACE_DYNAMO_TABLE=mind-palace-graph-dev
export MIND_PALACE_S3VECTORS_BUCKET=mind-palace-vectors-dev-086261225885
export MIND_PALACE_S3VECTORS_INDEX=wiki-pages
export VEIL_PORT=3001

# Prefer ACP so the agent already has VEIL teaching context (Tier 0/1):
export VEIL_MODEL_PROVIDER=acp
# Or Rig: ollama / openai (also gets wiki_* tools)

make serve PROJECT=/path/to/wear_test
```

When init succeeds, the log line is:

```text
Mind Palace tools enabled (dashlx AWS credentials)
```

### “store error: service error”

Almost always **wrong region** or **bad bucket name**:

| Symptom | Fix |
|---------|-----|
| `store error: service error` | Set `MIND_PALACE_REGION` to the stack region (`us-west-2` for dashlx_dev) |
| Table not found | Same — Dynamo is regional |
| Empty vector buckets | Same — S3 Vectors is regional |
| `s3://…` in env | Use bare name (or rely on strip) |

Verify:

```bash
export AWS_PROFILE=dashlx_dev
aws dynamodb describe-table --table-name mind-palace-graph-dev --region us-west-2
aws s3vectors list-indexes --vector-bucket-name mind-palace-vectors-dev-086261225885 --region us-west-2
```

## Who gets wiki tools?

| Path | How |
|------|-----|
| **ACP / Kiro** | MCP on `/api/mcp` via workspace `.kiro/settings/mcp.json` |
| **Rig (ollama/openai)** | `wiki_*` on the Rig agent in `prompt_with_tools` |

| Tool | Role |
|------|------|
| `wiki_search` | Semantic search |
| `wiki_read` | Progressive disclosure read |
| `wiki_traverse` | Graph neighbors |
| `wiki_create` / `wiki_update` | Write knowledge |
| `wiki_list` | List by type |

## Seed platform knowledge (via agent — not manually)

1. Serve with palace env + correct **region** (above).
2. In the Agent dock send: **`seed mind palace`**  
   Or: `./scripts/seed_mind_palace.sh`
3. Host expands that into a full seed task; agent uses `wiki_*`.

Target slugs: `veil-language-overview`, `veil-stubs-and-sdks`, `veil-bus-vs-rest`,
`veil-dual-loop`, `veil-ui-sveltekit5`, `sop-seed-and-extend-wiki`, `sop-add-cloud-adapter`.

## Environment promotion

Dev (`dashlx_dev`) is the training environment for now. S3 Vectors are not
easily exportable — promoting palace data across staging/prod is later ops.

## Aether UI

IDE agent dock uses **@aether-ui/core** ([jdwil/aether-ui](https://github.com/jdwil/aether-ui))
over WebSocket:

```
ws://{host}/api/chat
ws://{host}/api/p/{name}/chat
```

Install / refresh from GitHub (source exports — no vendor clone or `svelte-package`):

```bash
cd veil-viewer && npm install github:jdwil/aether-ui
```

`veil-viewer` depends on:

```json
"@aether-ui/core": "github:jdwil/aether-ui"
```

Tailwind v4 must `@source` the package (see `veil-viewer/src/app.css`):

```css
@source "../node_modules/@aether-ui/core/packages/aether-ui/src/lib";
```