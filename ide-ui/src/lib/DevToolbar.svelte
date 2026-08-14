<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { ideApiBase } from '$lib/store';

  interface Target {
    name: string;
    status: 'stopped' | 'starting' | 'running' | 'error' | 'generating';
    package: string;
    target: string;
    output: string;
    dev_command: string | null;
    dev_port: number | null;
    last_gen: string | null;
    last_error: string | null;
    attached: boolean;
  }

  /** Synthetic select value — API treats missing name as start/stop all. */
  const ALL = '__all__';

  let targets = $state<Target[]>([]);
  let selectedTarget = $state<string>(ALL);
  let showLogs = $state(false);
  let logs = $state<string[]>([]);
  let pollInterval: ReturnType<typeof setInterval> | null = null;
  let hasTargets = $state(false);

  function apiBase(): string {
    return ideApiBase();
  }

  function isAll(): boolean {
    return selectedTarget === ALL;
  }

  async function fetchTargets() {
    try {
      const resp = await fetch(`${apiBase()}/dev/targets`);
      const data = await resp.json();
      if (data.targets && data.targets.length > 0) {
        targets = data.targets;
        hasTargets = true;
        // Keep ALL or a still-valid name; default multi → ALL, single → that target
        if (selectedTarget !== ALL && !targets.some((t) => t.name === selectedTarget)) {
          selectedTarget = targets.length > 1 ? ALL : targets[0].name;
        }
      } else {
        hasTargets = false;
      }
    } catch {
      hasTargets = false;
    }
  }

  async function startTarget() {
    const body = isAll() ? {} : { name: selectedTarget };
    await fetch(`${apiBase()}/dev/start`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body)
    });
    await fetchTargets();
  }

  async function stopTarget() {
    const body = isAll() ? {} : { name: selectedTarget };
    await fetch(`${apiBase()}/dev/stop`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body)
    });
    await fetchTargets();
  }

  async function fetchLogs() {
    try {
      if (isAll()) {
        const resp = await fetch(`${apiBase()}/dev/logs`);
        const data = await resp.json();
        if (Array.isArray(data.targets)) {
          logs = data.targets.flatMap((t: { name: string; logs?: string[] }) => [
            `── ${t.name} ──`,
            ...(t.logs ?? [])
          ]);
        } else {
          logs = data.logs ?? [];
        }
      } else {
        if (!selectedTarget) return;
        const resp = await fetch(`${apiBase()}/dev/logs?name=${encodeURIComponent(selectedTarget)}`);
        const data = await resp.json();
        logs = data.logs ?? [];
      }
    } catch {
      logs = [];
    }
  }

  function statusColor(status: string): string {
    switch (status) {
      case 'running':
        return '#10b981';
      case 'starting':
      case 'generating':
        return '#f59e0b';
      case 'error':
        return '#ef4444';
      default:
        return '#6b7280';
    }
  }

  function currentTarget(): Target | undefined {
    return targets.find((t) => t.name === selectedTarget);
  }

  /** Aggregate status when "All" is selected. */
  function displayStatus(): string {
    if (!isAll()) return currentTarget()?.status ?? 'stopped';
    if (targets.some((t) => t.status === 'error')) return 'error';
    if (targets.some((t) => t.status === 'generating' || t.status === 'starting')) return 'starting';
    if (targets.length > 0 && targets.every((t) => t.status === 'running')) return 'running';
    if (targets.some((t) => t.status === 'running')) return 'starting'; // partial
    return 'stopped';
  }

  function isBusy(): boolean {
    const s = displayStatus();
    return s === 'running' || s === 'starting' || s === 'generating';
  }

  function runningPorts(): { name: string; port: number; attached: boolean }[] {
    const list = isAll() ? targets : targets.filter((t) => t.name === selectedTarget);
    return list
      .filter((t) => t.status === 'running' && t.dev_port)
      .map((t) => ({ name: t.name, port: t.dev_port as number, attached: t.attached }));
  }

  function displayError(): string | null {
    if (!isAll()) return currentTarget()?.last_error ?? null;
    const err = targets.find((t) => t.last_error)?.last_error;
    return err ?? null;
  }

  onMount(async () => {
    await fetchTargets();
    pollInterval = setInterval(fetchTargets, 3000);
  });

  onDestroy(() => {
    if (pollInterval) clearInterval(pollInterval);
  });
</script>

{#if hasTargets}
  <div class="dev-toolbar">
    <div class="dev-controls">
      <select class="dev-target-select" bind:value={selectedTarget} title="Dev target">
        {#if targets.length > 1}
          <option value={ALL}>All targets</option>
        {/if}
        {#each targets as t}
          <option value={t.name}>
            {t.name} ({t.target})
          </option>
        {/each}
      </select>

      <span
        class="dev-status-dot"
        style="background: {statusColor(displayStatus())}"
        title={displayStatus()}
      ></span>

      {#if isBusy()}
        <button class="dev-btn dev-stop" title={isAll() ? 'Stop all' : 'Stop dev server'} onclick={stopTarget}>
          ■
        </button>
      {:else}
        <button class="dev-btn dev-play" title={isAll() ? 'Start all' : 'Start dev server'} onclick={startTarget}>
          ▶
        </button>
      {/if}

      {#each runningPorts() as p}
        <a
          class="dev-port-link"
          href="http://127.0.0.1:{p.port}"
          target="_blank"
          title="Open {p.name}{p.attached ? ' (reattached)' : ''}"
        >
          {#if p.attached}<span class="dev-attached-badge" title="Reattached to existing server">🔗</span>{/if}
          {#if isAll()}{p.name}{/if}:{p.port}
        </a>
      {/each}

      <button
        class="dev-btn dev-logs-btn"
        class:active={showLogs}
        title="Toggle logs"
        onclick={() => {
          showLogs = !showLogs;
          if (showLogs) fetchLogs();
        }}
      >
        📋
      </button>
    </div>

    {#if displayError()}
      <div class="dev-error" title={displayError() ?? ''}>
        ⚠ {displayError()?.slice(0, 60)}…
      </div>
    {/if}
  </div>

  {#if showLogs}
    <div class="dev-logs-panel">
      <div class="dev-logs-header">
        <span>Logs: {isAll() ? 'all' : selectedTarget}</span>
        <button class="dev-btn" onclick={fetchLogs}>↻</button>
        <button class="dev-btn" onclick={() => (showLogs = false)}>✕</button>
      </div>
      <div class="dev-logs-body">
        {#each logs as line}
          <div class="dev-log-line">{line}</div>
        {:else}
          <div class="dev-log-line dim">No logs yet.</div>
        {/each}
      </div>
    </div>
  {/if}
{/if}

<style>
  .dev-toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
    background: var(--veil-surface, #1e1e2e);
    border-bottom: 1px solid var(--veil-border, #333);
    font-size: 12px;
  }

  .dev-controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .dev-target-select {
    background: var(--veil-input-bg, #2a2a3e);
    color: var(--veil-text, #e2e8f0);
    border: 1px solid var(--veil-border, #444);
    border-radius: 4px;
    padding: 2px 6px;
    font-size: 11px;
    cursor: pointer;
  }

  .dev-status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .dev-btn {
    background: var(--veil-input-bg, #2a2a3e);
    color: var(--veil-text, #e2e8f0);
    border: 1px solid var(--veil-border, #444);
    border-radius: 4px;
    padding: 2px 8px;
    font-size: 12px;
    cursor: pointer;
    line-height: 1;
  }

  .dev-btn:hover {
    background: var(--veil-accent-hover, #3a3a5e);
  }

  .dev-play {
    color: #10b981;
  }

  .dev-stop {
    color: #ef4444;
  }

  .dev-logs-btn.active {
    background: var(--veil-accent-hover, #3a3a5e);
    border-color: var(--veil-accent, #60a5fa);
  }

  .dev-port-link {
    font-size: 10px;
    color: #60a5fa;
    text-decoration: none;
    font-family: monospace;
  }

  .dev-port-link:hover {
    text-decoration: underline;
  }

  .dev-attached-badge {
    font-size: 9px;
    margin-right: 2px;
  }

  .dev-error {
    font-size: 10px;
    color: #f87171;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 300px;
  }

  .dev-logs-panel {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    height: 200px;
    background: var(--veil-surface, #1a1a2e);
    border-top: 1px solid var(--veil-border, #333);
    display: flex;
    flex-direction: column;
    z-index: 100;
  }

  .dev-logs-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 12px;
    background: var(--veil-input-bg, #2a2a3e);
    border-bottom: 1px solid var(--veil-border, #333);
    font-size: 11px;
    color: var(--veil-text-dim, #94a3b8);
  }

  .dev-logs-body {
    flex: 1;
    overflow-y: auto;
    padding: 8px 12px;
    font-family: 'JetBrains Mono', monospace;
    font-size: 10px;
    line-height: 1.5;
  }

  .dev-log-line {
    color: var(--veil-text-secondary, #cbd5e1);
    white-space: pre-wrap;
    word-break: break-all;
  }

  .dev-log-line.dim {
    color: var(--veil-text-faint, #64748b);
    font-style: italic;
  }
</style>
