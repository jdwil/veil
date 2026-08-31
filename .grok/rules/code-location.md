---
description: Where DashLX product code lives and how to modify it
globs: "**/*"
---

# Code Location Rules (Non-Negotiable)

## Product code lives in the runtime data center (S3/DDB)

ALL DashLX product code — VEIL source, terraform, config — is stored in the VEIL runtime and accessed via the runtime API. You MUST NOT create or edit local files for product code.

### To write product code:
```bash
curl -X POST "http://localhost:8080/api/write-file" \
  -H "Content-Type: application/json" \
  -d '{"repo_id":"<slug>","path":"<file-path>","content":"<content>","message":"<commit msg>"}'
```

### To read product code:
```bash
curl -X POST "http://localhost:8080/api/read-file" \
  -H "Content-Type: application/json" \
  -d '{"repo_id":"<slug>","path":"<file-path>"}'
```

## What goes where

| What | Where | How |
|------|-------|-----|
| VEIL engine (compiler, runtime, IDE) | This repo (`~/dev/jd/veil/`) | git |
| Layers (.layer files) | This repo (`~/dev/jd/veil/layers/`) | git |
| DashLX product source (main.veil, etc.) | Runtime S3 via API | `/api/write-file` |
| Product infrastructure (terraform) | Runtime S3, inside the project | `/api/write-file` path=`terraform/main.tf` |
| DashLX shared infra (VPC, ECS cluster) | `~/dev/dashlx/dlx-core/infra/` | terraform + git |

## NEVER do these things

- NEVER create `deploy/terraform/` in this repo. It was deleted. Product infra lives in VEIL projects.
- NEVER edit files in `/tmp/` or `/home/jd/tmp/veil-projects/` expecting them to persist. Those are stale.
- NEVER write product code as local files. The runtime API is the single source of truth.
- NEVER look in `~/dev/dashlx/dlx-core/infra/` for product-level resources (DLX AI CloudFront, Agent Core Lambda). Those are in the VEIL projects in S3.

## Project slugs in the runtime

- `dlx-ai` — Frontend harness (ai.dev.dashlx.com)
- `agent-core` — Agent/Team/Tool management service
- `agentic-workflows` — Workflow orchestration service
- `dlx-bus` — Service bus library
- `dlx-auth` — Auth library

## Project naming & structure conventions (NON-NEGOTIABLE)

- **Project NAME is title-case, SLUG is kebab-case.** Name = "Agent Core"; slug = "agent-core". Never name a project in kebab-case.
- **One project holds BOTH backend and UI.** Do NOT create a separate `*-ui` project for a project's interface.
- **Backend/domain → `main.veil`. User interface → `ui.veil`.** This is the VEIL convention. A project's frontend contribution is authored in `ui.veil`, generated to TypeScript; the backend is authored in `main.veil`, generated to Rust. `veil.toml` declares both targets.
- Example — the "Agent Core" project (slug `agent-core`):
  - `main.veil` → domain, ports, adapters, handlers → Rust Lambda
  - `ui.veil` → CrudResource pages → contribution bundle
  - one `veil.toml` with a rust target (main.veil) + a contribution target (ui.veil)

NEVER create `agent-core-ui`, `agentic-workflows-ui`, etc. UI goes in the project's `ui.veil`.
