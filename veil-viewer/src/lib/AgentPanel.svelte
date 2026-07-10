<script lang="ts">
  /**
   * AGT-001: In-IDE agent panel (MVP vertical slice).
   * Sends prompts to POST /api/agent/turn; refreshes IR when source changes.
   */
  import { fetchIr } from '$lib/store';

  interface AgentMessage {
    role: string;
    content: string;
  }
  interface ToolCall {
    name: string;
    detail: string;
  }
  interface TurnResponse {
    turn_id: string;
    messages: AgentMessage[];
    tool_calls: ToolCall[];
    source_changed: boolean;
    ok: boolean;
    error?: string | null;
    backend?: string;
    context_truncated?: boolean;
    context_warning?: string | null;
    context_tokens?: number;
    context_budget_tokens?: number;
    context_layers?: string[];
  }

  let open = $state(false);
  let prompt = $state('');
  let busy = $state(false);
  let err = $state<string | null>(null);
  let contextWarn = $state<string | null>(null);
  let contextMeta = $state<string>('');
  let history = $state<{ role: string; content: string; tools?: ToolCall[] }[]>([]);
  let abort: AbortController | null = null;

  async function send() {
    const text = prompt.trim();
    if (!text || busy) return;
    busy = true;
    err = null;
    contextWarn = null;
    history = [...history, { role: 'user', content: text }];
    prompt = '';
    abort = new AbortController();
    try {
      const res = await fetch('http://localhost:3001/api/agent/turn', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ prompt: text }),
        signal: abort.signal,
      });
      if (!res.ok) {
        err = `HTTP ${res.status}: ${await res.text()}`;
        return;
      }
      const data: TurnResponse = await res.json();
      if (data.context_truncated) {
        contextWarn =
          data.context_warning ||
          'Agent teaching context was truncated — model is unreliable. Switch model/ACP or raise VEIL_AGENT_PREAMBLE_MAX_TOKENS.';
      }
      const layers = (data.context_layers || []).join(', ') || '—';
      contextMeta = `ctx ≈${data.context_tokens ?? '?'} / ${data.context_budget_tokens ?? '?'} tok · layers: ${layers} · ${data.backend ?? ''}`;
      for (const m of data.messages.filter((x) => x.role === 'assistant')) {
        history = [
          ...history,
          { role: 'assistant', content: m.content, tools: data.tool_calls },
        ];
      }
      if (data.error) err = data.error;
      if (data.source_changed) {
        await fetchIr();
      }
    } catch (e: any) {
      if (e?.name === 'AbortError') {
        history = [...history, { role: 'assistant', content: '(cancelled)' }];
      } else {
        err = String(e);
      }
    } finally {
      busy = false;
      abort = null;
    }
  }

  function cancel() {
    abort?.abort();
  }
</script>

<div class="agent-wrap">
  <button type="button" class="agent-toggle" onclick={() => (open = !open)} title="Agent panel">
    {open ? '▾' : '▸'} Agent
  </button>
  {#if open}
    <div class="agent-panel">
      <div class="agent-head">
        <span class="title">Built-in agent (Rig)</span>
        <span class="hint">layer prompts + tools · offline: check · outline · rename</span>
      </div>
      {#if contextWarn}
        <div class="ctx-warn" role="alert">
          <strong>⚠️ Context truncated</strong>
          <pre class="ctx-warn-body">{contextWarn}</pre>
          <p class="ctx-warn-foot">
            Prefer a larger-context model, OpenAI flagship, or ACP — not a 9B with a cut curriculum.
          </p>
        </div>
      {/if}
      {#if contextMeta}
        <div class="ctx-meta">{contextMeta}</div>
      {/if}
      <div class="thread">
        {#each history as m}
          <div class="msg" class:user={m.role === 'user'} class:asst={m.role === 'assistant'}>
            <div class="role">{m.role}</div>
            <pre class="body">{m.content}</pre>
            {#if m.tools?.length}
              <div class="tools">
                {#each m.tools as t}
                  <span class="tool">{t.name}: {t.detail}</span>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
        {#if history.length === 0}
          <p class="empty">
            Free-form prompts use Ollama/OpenAI with Tier 0/1 layer teaching context. Offline:
            <code>check</code> · <code>outline</code> · <code>rename A to B</code>.
          </p>
        {/if}
      </div>
      {#if err}
        <div class="err">{err}</div>
      {/if}
      <div class="composer">
        <input
          type="text"
          bind:value={prompt}
          placeholder="Prompt…"
          disabled={busy}
          onkeydown={(e) => e.key === 'Enter' && send()}
        />
        {#if busy}
          <button type="button" class="cancel" onclick={cancel}>Cancel</button>
        {:else}
          <button type="button" class="send" onclick={send} disabled={!prompt.trim()}>Send</button>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .agent-wrap {
    position: relative;
  }
  .agent-toggle {
    background: none;
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    color: var(--veil-text-dim);
    font-size: 11px;
    padding: 4px 8px;
    cursor: pointer;
  }
  .agent-panel {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    width: min(480px, 94vw);
    max-height: 560px;
    background: var(--veil-surface);
    border: 1px solid var(--veil-border);
    border-radius: 8px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45);
    z-index: 60;
    display: flex;
    flex-direction: column;
  }
  .agent-head {
    display: flex;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--veil-border);
    font-size: 11px;
  }
  .title {
    font-weight: 700;
  }
  .hint {
    color: var(--veil-text-faint);
    font-size: 10px;
  }
  .ctx-warn {
    background: rgba(248, 113, 113, 0.12);
    border-bottom: 1px solid #f87171;
    padding: 8px 12px;
    color: #fecaca;
    font-size: 11px;
  }
  .ctx-warn-body {
    margin: 6px 0 0;
    white-space: pre-wrap;
    font-size: 10px;
    font-family: 'JetBrains Mono', monospace;
    max-height: 140px;
    overflow: auto;
  }
  .ctx-warn-foot {
    margin: 6px 0 0;
    font-size: 10px;
    color: #fca5a5;
  }
  .ctx-meta {
    font-size: 9px;
    color: var(--veil-text-faint);
    padding: 4px 12px;
    border-bottom: 1px solid var(--veil-border);
    font-family: 'JetBrains Mono', monospace;
  }
  .thread {
    flex: 1;
    overflow: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 160px;
    max-height: 320px;
  }
  .msg {
    border-radius: 6px;
    padding: 6px 8px;
    background: var(--veil-input-bg, rgba(0, 0, 0, 0.2));
  }
  .msg.user {
    border-left: 2px solid #60a5fa;
  }
  .msg.asst {
    border-left: 2px solid #4ade80;
  }
  .role {
    font-size: 9px;
    text-transform: uppercase;
    color: var(--veil-text-faint);
    margin-bottom: 2px;
  }
  .body {
    margin: 0;
    white-space: pre-wrap;
    font-size: 11px;
    font-family: 'JetBrains Mono', monospace;
    color: var(--veil-text);
  }
  .tools {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 6px;
  }
  .tool {
    font-size: 9px;
    padding: 1px 6px;
    border-radius: 4px;
    background: rgba(96, 165, 250, 0.15);
    color: #93c5fd;
  }
  .composer {
    display: flex;
    gap: 6px;
    padding: 8px;
    border-top: 1px solid var(--veil-border);
  }
  .composer input {
    flex: 1;
    border: 1px solid var(--veil-border);
    border-radius: 4px;
    background: var(--veil-surface-alt);
    color: var(--veil-text);
    padding: 6px 8px;
    font-size: 12px;
  }
  .send,
  .cancel {
    border: 1px solid var(--veil-border);
    border-radius: 4px;
    background: var(--veil-accent-subtle);
    color: var(--veil-text);
    font-size: 11px;
    padding: 4px 10px;
    cursor: pointer;
  }
  .cancel {
    color: #f87171;
  }
  .err {
    color: #f87171;
    font-size: 11px;
    padding: 4px 12px;
  }
  .empty {
    font-size: 11px;
    color: var(--veil-text-faint);
    font-style: italic;
    margin: 12px;
  }
</style>
