<script lang="ts">
  import {
    diagnostics,
    checkMeta,
    focusDiagnostic,
    type Diagnostic,
  } from '$lib/store';

  let expanded = $state(false);
  const items = $derived($diagnostics);
  const meta = $derived($checkMeta);
  const count = $derived(items.length);
  const errorCount = $derived(
    items.filter((d) => d.severity === 'Error' || d.severity === 'error').length
  );
  const warningCount = $derived(count - errorCount);

  function label(diag: Diagnostic): string {
    const code = diag.code ? `[${diag.code}] ` : '';
    const where = diag.node_name ? `${diag.node_name}: ` : '';
    return `${code}${where}${diag.message}`;
  }

  function onSelect(diag: Diagnostic) {
    focusDiagnostic(diag);
  }

  function badgeText(): string {
    if (errorCount > 0 && warningCount > 0) {
      return `${errorCount} error${errorCount === 1 ? '' : 's'}, ${warningCount} warning${warningCount === 1 ? '' : 's'}`;
    }
    if (errorCount > 0) {
      return `${errorCount} error${errorCount === 1 ? '' : 's'}`;
    }
    return `${warningCount} warning${warningCount === 1 ? '' : 's'}`;
  }
</script>

{#if count > 0}
  <div class="diagnostics-badge" class:expanded class:has-errors={errorCount > 0}>
    <button class="badge-btn" onclick={() => (expanded = !expanded)}>
      {errorCount > 0 ? '⛔' : '⚠️'}
      {badgeText()}
    </button>

    {#if expanded}
      <div class="diagnostics-list">
        {#if meta}
          <div class="diag-meta">
            target: {meta.target}
            {#if meta.escape_hatch}
              · escape debt: {(meta.escape_hatch.raw_surface ?? 0) +
                (meta.escape_hatch.empty_adapter ?? 0) +
                (meta.escape_hatch.external_call ?? 0) +
                (meta.escape_hatch.json_boundary ?? 0)}
            {/if}
          </div>
        {/if}
        {#each items as diag}
          <button
            type="button"
            class="diag-item"
            class:error={diag.severity === 'Error' || diag.severity === 'error'}
            class:clickable={diag.node_id != null || !!diag.node_name}
            onclick={() => onSelect(diag)}
            title={diag.hint ?? (diag.node_id != null ? `Go to node ${diag.node_id}` : 'Select related node')}
          >
            <span class="diag-severity"
              >{diag.severity === 'Error' || diag.severity === 'error' ? '🔴' : '🟡'}</span
            >
            <span class="diag-message">{label(diag)}</span>
            {#if diag.hint}
              <span class="diag-hint">{diag.hint}</span>
            {/if}
          </button>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .diagnostics-badge {
    position: absolute;
    top: 12px;
    left: 12px;
    right: auto;
    bottom: auto;
    z-index: 30;
    font-family: var(--veil-font, system-ui);
    max-width: min(420px, calc(100% - 80px));
  }

  .badge-btn {
    background: var(--veil-surface, #1e1e1e);
    border: 1px solid #f59e0b;
    color: #f59e0b;
    padding: 6px 12px;
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.8em;
    font-weight: 600;
  }
  .has-errors .badge-btn {
    border-color: #ef4444;
    color: #ef4444;
  }
  .badge-btn:hover {
    background: #292524;
  }

  .diagnostics-list {
    margin-top: 8px;
    background: var(--veil-surface, #1e1e1e);
    border: 1px solid var(--veil-border, #333);
    border-radius: 8px;
    padding: 8px;
    max-height: 360px;
    overflow-y: auto;
    min-width: 320px;
    max-width: 420px;
  }

  .diag-meta {
    font-size: 0.7em;
    color: var(--veil-text-dim, #737373);
    padding: 4px 8px 8px;
    border-bottom: 1px solid var(--veil-border, #333);
    margin-bottom: 4px;
  }

  .diag-item {
    display: block;
    width: 100%;
    text-align: left;
    padding: 6px 8px;
    border: none;
    border-bottom: 1px solid var(--veil-border, #333);
    background: transparent;
    font-size: 0.8em;
    color: var(--veil-text, #e5e5e5);
    font-family: inherit;
  }
  .diag-item:last-child {
    border-bottom: none;
  }
  .diag-item.clickable {
    cursor: pointer;
  }
  .diag-item.clickable:hover {
    background: #292524;
  }
  .diag-item.error .diag-message {
    color: #fca5a5;
  }

  .diag-severity {
    margin-right: 6px;
  }

  .diag-message {
    color: var(--veil-text-dim, #a3a3a3);
  }

  .diag-hint {
    display: block;
    margin-top: 2px;
    margin-left: 1.4em;
    font-size: 0.9em;
    color: #737373;
    font-style: italic;
  }
</style>
