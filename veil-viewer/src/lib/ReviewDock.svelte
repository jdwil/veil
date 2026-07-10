<script lang="ts">
  /**
   * Bottom review dock — tabs for VEIL source + Agent (canvas stays full width).
   * Foundation for IDE-style "click to insert context" into the agent prompt.
   */
  import VeilSourcePanel from './VeilSourcePanel.svelte';
  import AgentPanel from './AgentPanel.svelte';
  import { selectedNodeId, irGraph } from '$lib/store';
  import { get } from 'svelte/store';

  type DockTab = 'source' | 'agent';

  let tab = $state<DockTab>('source');
  let expanded = $state(true);
  let agentInsert = $state('');

  /** Selection chip for agent "insert context". */
  let selectionLabel = $derived.by(() => {
    const sid = $selectedNodeId;
    const g = $irGraph;
    if (!sid || !g) return null;
    const n = g.nodes.find((x) => String(x.id) === sid);
    if (!n) return null;
    const kind = n.metadata.subkind || n.kind;
    return `${kind} ${n.name}`;
  });

  function insertSelection() {
    if (!selectionLabel) return;
    agentInsert = selectionLabel;
    tab = 'agent';
    // clear after AgentPanel consumes (via bind or tick)
    queueMicrotask(() => {
      agentInsert = '';
    });
  }
</script>

<div class="review-dock" class:collapsed={!expanded}>
  <div class="dock-chrome">
    <div class="dock-tabs" role="tablist" aria-label="Review panels">
      <button
        type="button"
        role="tab"
        class="dock-tab"
        class:active={tab === 'source'}
        aria-selected={tab === 'source'}
        onclick={() => {
          expanded = true;
          tab = 'source';
        }}
      >
        VEIL source
      </button>
      <button
        type="button"
        role="tab"
        class="dock-tab"
        class:active={tab === 'agent'}
        aria-selected={tab === 'agent'}
        onclick={() => {
          expanded = true;
          tab = 'agent';
        }}
      >
        Agent
      </button>
    </div>
    <div class="dock-actions">
      {#if selectionLabel}
        <button
          type="button"
          class="insert-btn"
          title="Insert selected construct into agent prompt"
          onclick={insertSelection}
        >
          + Insert “{selectionLabel}”
        </button>
      {/if}
      <button
        type="button"
        class="collapse-btn"
        title={expanded ? 'Collapse dock' : 'Expand dock'}
        onclick={() => (expanded = !expanded)}
      >
        {expanded ? '▾' : '▴'}
      </button>
    </div>
  </div>
  {#if expanded}
    <div class="dock-body">
      {#if tab === 'source'}
        <VeilSourcePanel embedded />
      {:else}
        <AgentPanel embedded insertToken={agentInsert} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .review-dock {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--veil-border);
    background: var(--veil-surface);
    max-height: 42vh;
    min-height: 0;
    z-index: 5;
  }
  .review-dock.collapsed {
    max-height: none;
  }
  .dock-chrome {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 0 8px;
    border-bottom: 1px solid var(--veil-border);
    flex-shrink: 0;
    background: var(--veil-surface-alt);
  }
  .dock-tabs {
    display: flex;
    gap: 2px;
  }
  .dock-tab {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--veil-text-dim);
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.03em;
    padding: 8px 12px;
    cursor: pointer;
  }
  .dock-tab:hover {
    color: var(--veil-text);
  }
  .dock-tab.active {
    color: var(--veil-text);
    border-bottom-color: var(--veil-accent, #60a5fa);
  }
  .dock-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .insert-btn {
    font-size: 10px;
    padding: 3px 8px;
    border-radius: 4px;
    border: 1px solid var(--veil-border);
    background: var(--veil-accent-subtle);
    color: var(--veil-text-secondary);
    cursor: pointer;
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .insert-btn:hover {
    color: var(--veil-text);
    border-color: var(--veil-accent, #60a5fa);
  }
  .collapse-btn {
    background: none;
    border: none;
    color: var(--veil-text-dim);
    cursor: pointer;
    font-size: 12px;
    padding: 4px 6px;
  }
  .dock-body {
    flex: 1;
    min-height: 160px;
    max-height: calc(42vh - 36px);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
</style>
