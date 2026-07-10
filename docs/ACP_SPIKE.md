# ACP spike (AGT-007) — go / no-go

**Date:** 2026-07-10  
**Context:** VEIL IDE agent is **Rig-based** with tools over `SourceProvider`.
We evaluated attaching external **ACP** (Agent Client Protocol) agents.

## Research questions

### 1. Can we inject VEIL MCP tools into an ACP session?

**Partial.** ACP sessions typically expose the **host workspace filesystem**
and the agent’s own tool surface. Injecting *our* JSON-schema tools into a
third-party ACP agent is not standardized across Claude Code / Zed / Gemini.

**VEIL path today:** `GET /api/agent/tools` (veil-tools-v1) + Rig-hosted tools
in-process. An MCP bridge can wrap these (AGT-008 follow-up with `rig-mcp` /
`rmcp`). ACP agents that speak MCP can then load VEIL tools **as MCP**, not
as native ACP tools.

### 2. Best degradation if ACP tool injection fails?

| Pattern | Pros | Cons |
|---------|------|------|
| **A. Prefer Rig built-in** (default) | Same tools, live IDE refresh, confirm-writes | Not “Claude Code inside panel” |
| **B. Workspace FS + external agent** | Full ACP UX; edits `.veil` on disk | No structured EditOp; need AGT-002 watch |
| **C. ACP↔MCP bridge process** | External agents get our tools | Extra process; auth/session binding |

**Chosen default:** **A** for product loop; **B** documented for power users
(`veil serve` + external agent on the same project root). **C** when MCP
stdio server lands.

### 3. Need a thin ACP↔MCP bridge?

**Yes for full ACP product UX**, not for dual-loop MVP. Sequence:

1. ~~Rig tools + heuristic~~ **done**  
2. ~~HTTP tool discovery~~ **done** (`/api/agent/tools`)  
3. MCP stdio server wrapping those tools (`rig-mcp` / `rmcp`)  
4. Optional ACP host that launches external agents with MCP config  

## Go / no-go

| Decision | |
|----------|--|
| **Go** on Rig + MCP tools as the portable agent tool surface | Yes |
| **Go** on ACP host as a **follow-on** after MCP stdio | Yes (not blocking dual loop) |
| **No-go** on blocking product on full ACP inject | Yes — do not wait |

## Config sketch (future)

```bash
# Built-in Rig (today)
VEIL_MODEL_PROVIDER=openai   # or ollama

# Future ACP launcher (not implemented)
# VEIL_ACP_COMMAND="path/to/agent"
# VEIL_ACP_MCP_URL="http://localhost:3001/api/agent/tools"
```

## Clients noted

| Client | MCP | ACP | VEIL integration path |
|--------|-----|-----|------------------------|
| Claude Code | yes | partial | MCP tools when server exists |
| Cursor | yes | no | MCP |
| Zed agent | yes | ACP-native | ACP host later; MCP today |
| Gemini CLI | varies | no | MCP / FS |

## Conclusion

Ship **Rig-first** agents. Treat ACP as a **client adapter** that consumes the
same tools via MCP, not a second edit pipeline.
