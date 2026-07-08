<script lang="ts">
  import { diagnostics } from '$lib/store';

  let expanded = $state(false);
  const items = $derived($diagnostics);
  const count = $derived(items.length);
</script>

{#if count > 0}
  <div class="diagnostics-badge" class:expanded>
    <button class="badge-btn" onclick={() => expanded = !expanded}>
      ⚠️ {count} {count === 1 ? 'warning' : 'warnings'}
    </button>

    {#if expanded}
      <div class="diagnostics-list">
        {#each items as diag}
          <div class="diag-item">
            <span class="diag-severity">{diag.severity === 'Error' ? '🔴' : '🟡'}</span>
            <span class="diag-message">{diag.message}</span>
          </div>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .diagnostics-badge {
    position: absolute;
    top: 12px;
    right: 12px;
    z-index: 10;
    font-family: var(--veil-font, system-ui);
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
  .badge-btn:hover {
    background: #292524;
  }

  .diagnostics-list {
    margin-top: 8px;
    background: var(--veil-surface, #1e1e1e);
    border: 1px solid var(--veil-border, #333);
    border-radius: 8px;
    padding: 8px;
    max-height: 300px;
    overflow-y: auto;
    min-width: 280px;
  }

  .diag-item {
    padding: 6px 8px;
    border-bottom: 1px solid var(--veil-border, #333);
    font-size: 0.8em;
    color: var(--veil-text, #e5e5e5);
  }
  .diag-item:last-child {
    border-bottom: none;
  }

  .diag-severity {
    margin-right: 6px;
  }

  .diag-message {
    color: var(--veil-text-dim, #a3a3a3);
  }
</style>
