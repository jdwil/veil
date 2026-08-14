<script lang="ts">
  /**
   * Built-in agent panel — streams assistant text (SSE typewriter).
   */
  import { untrack } from 'svelte';
  import { get } from 'svelte/store';
  import { checkMeta, ideApiBase, refreshAfterEdit } from '$lib/store';

  interface ToolCall {
    name: string;
    detail: string;
  }
  interface TurnResponse {
    turn_id: string;
    messages: { role: string; content: string }[];
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

  interface Props {
    embedded?: boolean;
    insertToken?: string;
  }
  let { embedded = false, insertToken = '' }: Props = $props();

  let prompt = $state('');
  let busy = $state(false);
  let err = $state<string | null>(null);
  let contextWarn = $state<string | null>(null);
  let contextMeta = $state<string>('');
  let syncNote = $state<string | null>(null);
  let statusLine = $state<string>('');
  let history = $state<{ role: string; content: string; tools?: ToolCall[]; streaming?: boolean }[]>(
    []
  );
  let abort: AbortController | null = null;
  let inputEl: HTMLTextAreaElement | null = $state(null);
  let threadEl: HTMLDivElement | null = $state(null);

  $effect(() => {
    const token = insertToken;
    if (!token) return;
    untrack(() => {
      const cur = prompt;
      const sep = cur && !cur.endsWith(' ') && !cur.endsWith('\n') ? ' ' : '';
      prompt = `${cur}${sep}\`${token}\``;
    });
    queueMicrotask(() => inputEl?.focus());
  });

  function scrollThread() {
    queueMicrotask(() => {
      if (threadEl) threadEl.scrollTop = threadEl.scrollHeight;
    });
  }

  function appendAssistantChunk(text: string) {
    const last = history[history.length - 1];
    if (last?.role === 'assistant' && last.streaming) {
      history = [
        ...history.slice(0, -1),
        { ...last, content: last.content + text },
      ];
    } else {
      history = [
        ...history,
        { role: 'assistant', content: text, streaming: true, tools: [] },
      ];
    }
    scrollThread();
  }

  function finalizeAssistant(tools?: ToolCall[]) {
    const last = history[history.length - 1];
    if (last?.role === 'assistant' && last.streaming) {
      history = [
        ...history.slice(0, -1),
        { ...last, streaming: false, tools: tools?.length ? tools : last.tools },
      ];
    }
  }

  async function send() {
    const text = prompt.trim();
    if (!text || busy) return;
    busy = true;
    err = null;
    contextWarn = null;
    syncNote = null;
    statusLine = 'connecting…';
    history = [...history, { role: 'user', content: text }];
    // Placeholder assistant bubble for streaming
    history = [...history, { role: 'assistant', content: '', streaming: true, tools: [] }];
    prompt = '';
    abort = new AbortController();
    scrollThread();

    try {
      const turnStreamUrl = `${ideApiBase()}/agent/turn/stream`;
      let res: Response;
      try {
        res = await fetch(turnStreamUrl, {
          method: 'POST',
          headers: {
            'Content-Type': 'application/json',
            Accept: 'text/event-stream',
          },
          body: JSON.stringify({ prompt: text }),
          signal: abort.signal,
        });
      } catch (netErr: any) {
        // Connection/CORS failure — try non-stream once with clear API URL in errors
        statusLine = 'stream unavailable, falling back…';
        await sendNonStream(text);
        return;
      }
      if (!res.ok) {
        // Fallback to non-streaming turn
        await sendNonStream(text);
        return;
      }
      if (!res.body) {
        err = 'No response body for stream';
        finalizeAssistant();
        return;
      }

      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = '';
      let tools: ToolCall[] = [];
      let donePayload: TurnResponse | null = null;

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        // SSE events separated by blank line
        const parts = buffer.split('\n\n');
        buffer = parts.pop() ?? '';
        for (const part of parts) {
          if (!part.trim() || part.startsWith(':')) continue; // keep-alive ping
          let eventName = 'message';
          const dataLines: string[] = [];
          for (const line of part.split('\n')) {
            if (line.startsWith('event:')) eventName = line.slice(6).trim();
            else if (line.startsWith('data:')) dataLines.push(line.slice(5).trimStart());
          }
          const dataStr = dataLines.join('\n');
          if (!dataStr) continue;
          let data: any = {};
          try {
            data = JSON.parse(dataStr);
          } catch {
            data = { text: dataStr };
          }

          if (eventName === 'status') {
            statusLine = data.message || statusLine;
          } else if (eventName === 'chunk') {
            const t = data.text ?? '';
            if (t) appendAssistantChunk(t);
          } else if (eventName === 'tool') {
            tools = [...tools, { name: data.name || 'tool', detail: data.detail || '' }];
            const last = history[history.length - 1];
            if (last?.role === 'assistant') {
              history = [
                ...history.slice(0, -1),
                { ...last, tools: [...tools] },
              ];
            }
          } else if (eventName === 'done') {
            donePayload = data as TurnResponse;
          } else if (eventName === 'error') {
            err = data.message || 'stream error';
          }
        }
      }

      finalizeAssistant(tools);

      if (donePayload) {
        if (donePayload.context_truncated) {
          contextWarn =
            donePayload.context_warning ||
            'Agent teaching context was truncated — model is unreliable.';
        }
        const layers = (donePayload.context_layers || []).join(', ') || '—';
        contextMeta = `ctx ≈${donePayload.context_tokens ?? '?'} / ${donePayload.context_budget_tokens ?? '?'} tok · layers: ${layers} · ${donePayload.backend ?? ''}`;
        if (donePayload.error) err = donePayload.error;
        // If stream never sent chunks, fill from done payload
        const last = history[history.length - 1];
        if (last?.role === 'assistant' && !last.content) {
          const asst = donePayload.messages?.filter((m) => m.role === 'assistant').pop();
          if (asst) {
            history = [
              ...history.slice(0, -1),
              {
                role: 'assistant',
                content: asst.content,
                tools: donePayload.tool_calls || tools,
                streaming: false,
              },
            ];
          }
        } else if (donePayload.tool_calls?.length) {
          finalizeAssistant(donePayload.tool_calls);
        }
        if (donePayload.source_changed) {
          await refreshAfterEdit();
          const meta = get(checkMeta);
          syncNote = `Source applied · live check: ${meta?.error_count ?? '?'} error(s), ${meta?.warning_count ?? '?'} warning(s)`;
        }
      }
      statusLine = '';
    } catch (e: any) {
      if (e?.name === 'AbortError') {
        finalizeAssistant();
        appendAssistantChunk('\n(cancelled)');
        finalizeAssistant();
      } else {
        err = String(e);
        finalizeAssistant();
      }
      statusLine = '';
    } finally {
      busy = false;
      abort = null;
      scrollThread();
    }
  }

  /** Fallback when stream endpoint unavailable. */
  async function sendNonStream(text: string) {
    // Remove empty streaming bubble
    if (history[history.length - 1]?.streaming) {
      history = history.slice(0, -1);
    }
    const turnUrl = `${ideApiBase()}/agent/turn`;
    let res: Response;
    try {
      res = await fetch(turnUrl, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ prompt: text }),
        signal: abort?.signal,
      });
    } catch (netErr: any) {
      err =
        netErr?.name === 'AbortError'
          ? 'cancelled'
          : `Network error talking to agent at ${turnUrl} — is the API up? (${netErr?.message || netErr})`;
      return;
    }
    if (!res.ok) {
      err = `HTTP ${res.status} (${turnUrl}): ${await res.text()}`;
      return;
    }
    const data: TurnResponse = await res.json();
    // Typewriter client-side
    history = [...history, { role: 'assistant', content: '', streaming: true, tools: [] }];
    const full =
      data.messages?.filter((m) => m.role === 'assistant').map((m) => m.content).join('\n\n') ||
      '';
    for (const ch of full) {
      if (abort?.signal.aborted) break;
      appendAssistantChunk(ch);
      await new Promise((r) => setTimeout(r, 8));
    }
    finalizeAssistant(data.tool_calls);
    if (data.context_truncated) {
      contextWarn = data.context_warning || 'Context truncated';
    }
    const layers = (data.context_layers || []).join(', ') || '—';
    contextMeta = `ctx ≈${data.context_tokens ?? '?'} / ${data.context_budget_tokens ?? '?'} tok · layers: ${layers} · ${data.backend ?? ''}`;
    if (data.error) err = data.error;
    if (data.source_changed) {
      await refreshAfterEdit();
      const meta = get(checkMeta);
      syncNote = `Source applied · live check: ${meta?.error_count ?? '?'} error(s), ${meta?.warning_count ?? '?'} warning(s)`;
    }
  }

  function cancel() {
    abort?.abort();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  }
</script>

<div class="agent-panel" class:embedded>
  {#if !embedded}
    <div class="agent-head">
      <span class="title">Agent</span>
      <span class="hint">streaming · layer context</span>
    </div>
  {/if}
  {#if contextWarn}
    <div class="ctx-warn" role="alert">
      <strong>⚠️ Context truncated</strong>
      <pre class="ctx-warn-body">{contextWarn}</pre>
    </div>
  {/if}
  {#if contextMeta}
    <div class="ctx-meta">{contextMeta}</div>
  {/if}
  {#if statusLine}
    <div class="status-line" role="status">{statusLine}</div>
  {/if}
  {#if syncNote}
    <div class="sync-note" role="status">{syncNote}</div>
  {/if}
  <div class="thread" bind:this={threadEl}>
    {#each history as m}
      <div class="msg" class:user={m.role === 'user'} class:asst={m.role === 'assistant'}>
        <div class="role">
          {m.role}{#if m.streaming}<span class="cursor" aria-hidden="true">▍</span>{/if}
        </div>
        <pre class="body">{m.content}{#if m.streaming}<span class="blink">▌</span>{/if}</pre>
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
        Responses stream live (typewriter). Select a node and use <strong>+ Insert</strong>.
        Shift+Enter for newline.
      </p>
    {/if}
  </div>
  {#if err}
    <div class="err">{err}</div>
  {/if}
  <div class="composer">
    <textarea
      bind:this={inputEl}
      bind:value={prompt}
      placeholder="Ask the agent… (streamed reply)"
      rows="2"
      disabled={busy}
      onkeydown={onKey}
    ></textarea>
    {#if busy}
      <button type="button" class="cancel" onclick={cancel}>Cancel</button>
    {:else}
      <button type="button" class="send" onclick={send} disabled={!prompt.trim()}>Send</button>
    {/if}
  </div>
</div>

<style>
  .agent-panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
    background: var(--veil-surface);
  }
  .agent-panel.embedded {
    height: 100%;
  }
  .agent-head {
    display: flex;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--veil-border);
    font-size: 11px;
    flex-shrink: 0;
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
    flex-shrink: 0;
  }
  .ctx-warn-body {
    margin: 6px 0 0;
    white-space: pre-wrap;
    font-size: 10px;
    font-family: 'JetBrains Mono', monospace;
    max-height: 100px;
    overflow: auto;
  }
  .ctx-meta {
    font-size: 9px;
    color: var(--veil-text-faint);
    padding: 4px 12px;
    border-bottom: 1px solid var(--veil-border);
    font-family: 'JetBrains Mono', monospace;
    flex-shrink: 0;
  }
  .status-line {
    font-size: 10px;
    color: #93c5fd;
    padding: 4px 12px;
    border-bottom: 1px solid var(--veil-border);
    font-family: 'JetBrains Mono', monospace;
    flex-shrink: 0;
  }
  .sync-note {
    font-size: 10px;
    padding: 4px 12px;
    background: rgba(74, 222, 128, 0.12);
    color: #86efac;
    border-bottom: 1px solid rgba(74, 222, 128, 0.35);
    flex-shrink: 0;
  }
  .thread {
    flex: 1;
    overflow: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 80px;
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
  .blink {
    animation: blink 0.9s step-end infinite;
    color: #4ade80;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
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
    flex-shrink: 0;
    align-items: flex-end;
  }
  .composer textarea {
    flex: 1;
    border: 1px solid var(--veil-border);
    border-radius: 4px;
    background: var(--veil-surface-alt);
    color: var(--veil-text);
    padding: 6px 8px;
    font-size: 12px;
    font-family: inherit;
    resize: vertical;
    min-height: 40px;
    max-height: 120px;
    line-height: 1.35;
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
    flex-shrink: 0;
  }
  .empty {
    font-size: 11px;
    color: var(--veil-text-faint);
    font-style: italic;
    margin: 12px;
  }
</style>
