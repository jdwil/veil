# veil-agent-chat-attachments

**Type:** Leaf  
**Summary:** Drag/drop or paste one or more documents onto the runtime AgentDock; the agent receives text inlined and images as ACP vision blocks.

## Contract

- Operator can drop **multiple** files on the **whole agent pane** (not only the composer strip) or use the paperclip / paste.
- Supported for reading:
  - **Text / vector diagrams:** `.md`, `.veil`, `.drawio`, `.svg`, `.excalidraw`, mermaid, plantuml, JSON/XML, source.
  - **Raster diagrams / ERD screenshots:** `png` / `jpg` / `webp` / `gif` — sent as ACP `image` content blocks on the same turn.
  - **PDF / Office / other binary:** persisted under `$TMP/veil-chat-attachments/{turn}/` and named in the prompt so the agent can read the path.
- Client inlines text once under `# Attached documents`. The host must **not** double-inline when that heading is already present.
- Follow-up turns keep the inlined text via `message.metadata.wireText` (not just the short bubble label).
- Caps: 12 files, 8MB each, ~400KB text inline per file, ~16MB raster total.

## Do not

- Discard `File[]` from ChatInput (`text-only for now` is forbidden).
- Require the operator to re-type or paste document contents.
- Hand-edit generated customer Svelte for this — AgentDock lives in `ui/src/lib/agent/` (host, handwritten).

**Source of truth:** `ui/src/lib/agent/attachments.ts`, `crates/veil-server/src/chat_attachments.rs`
