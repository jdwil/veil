# VEIL

**Visual Engineering Intermediate Language** — a token-efficient IR that agents
author and humans oversee.

VEIL is not low-code and not “just another LLM prompt format.” It is a small,
stable language with layers (domain vocabulary at runtime), codegen backends
that emit real projects, and a visual runtime where authorship is visible.

This repository is **alpha**. The language, ProductHost, and IDE work well
enough to build with. APIs, UX, and docs will keep moving until beta.

Product intent lives in [`MISSION.md`](MISSION.md).

## What you can do

| Use | How | Cost |
|-----|-----|------|
| **Desktop VEIL** | `veil check` / `veil gen` / `veil serve` on your machine | Free (AGPL) |
| **Self-host ProductHost** | Run `veil-runtime` against your own AWS (or local disk) | Free (AGPL) |
| **Hosted VEIL Runtime** | We run ProductHost for you | Commercial — [contact](mailto:jd@unsung-operators.com) |
| **Commercial license** | Proprietary embedding or hosted use without AGPL obligations | Commercial — same address |

## License

**[AGPL-3.0-only](LICENSE)** — Copyright © 2026 [JD Williams](mailto:jd@unsung-operators.com)
(Unsung Operators).

Why AGPL and not GPL? The product we sell is a **network service**. GPLv3
does not require someone who hosts a modified runtime to share their source.
AGPL does. That keeps the commons intact and makes a commercial license
meaningful for organizations that cannot run AGPL in production.

See [`CONTRIBUTING.md`](CONTRIBUTING.md) before sending patches (inbound =
outbound AGPL unless a CLA is in place).

## Quick start

```bash
# Compiler
cargo build --release -p veil-cli
./target/release/veil check examples/hello_world.veil
./target/release/veil gen examples/hello_world.veil -t rust

# Single-project IDE (package authors)
./target/release/veil serve examples/hello_world.veil

# ProductHost (projects, git-on-S3, agent, review / sign-off)
cp .env.example .env   # then fill in AWS_* / table / bucket
cargo build --release -p veil-runtime
cd ui && npm install && cd ..
scripts/dev-stack.sh restart   # API :8080 + UI :5180
scripts/dev-stack.sh smoke
```

Do **not** run a second `veil serve --multi` next to ProductHost.

## Layout

```
crates/veil-parser     language front-end
crates/veil-ir         AST / IR / layers / stubs
crates/veil-codegen    rust + typescript backends
crates/veil-cli        veil check | gen | serve
crates/veil-server     IDE kernel, MCP, sessions, agent tools
crates/veil-runtime    ProductHost binary
ui/                    ProductHost SPA + in-shell IDE
layers/  stubs/        platform language packs
docs/                  architecture and ADRs
```

The host is handwritten Rust + Svelte. **Customer products** are authored in
`.veil` / `.layer` / `.stub`. Never hand-edit VEIL-generated outputs.

Git is history (S3 origin, local session checkouts). Outstanding changes and
human sign-off are a product surface, not a second VCS.

## Status

Alpha. Visible agency, git-as-history, and recorded sign-off are in ProductHost.
Expressiveness parity and additional codegen targets are the long road.

Questions and commercial inquiries: **jd@unsung-operators.com**.
