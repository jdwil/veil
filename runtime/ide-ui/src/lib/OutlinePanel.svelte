<script lang="ts">
  /** UX-014: outline / search jump for IR constructs. */
  import { irGraph, focusDiagnostic } from '$lib/store';
  import type { IrNode } from '$lib/types';

  let open = $state(false);
  let q = $state('');
  let searchEl: HTMLInputElement | undefined = $state();

  let items = $derived.by(() => {
    const g = $irGraph;
    if (!g) return [] as IrNode[];
    const needle = q.trim().toLowerCase();
    return g.nodes
      .filter((n) => n.kind !== 'Solution' && n.kind !== 'Action')
      .filter((n) => !n.metadata.annotations.includes('layer-provided'))
      .filter((n) => {
        if (!needle) return true;
        const sk = n.metadata.subkind ?? '';
        return (
          n.name.toLowerCase().includes(needle) ||
          sk.toLowerCase().includes(needle) ||
          n.kind.toLowerCase().includes(needle)
        );
      })
      .slice(0, 80);
  });

  function jump(node: IrNode) {
    focusDiagnostic({
      severity: 'Warning',
      message: 'outline jump',
      node_id: node.id,
      node_name: node.name,
    });
    open = false;
    q = '';
  }

  function onWindowKey(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      open = true;
      queueMicrotask(() => searchEl?.focus());
    }
    if (e.key === 'Escape' && open) {
      open = false;
      q = '';
    }
  }
</script>

<svelte:window onkeydown={onWindowKey} />

<div class="outline">
  <button
    type="button"
    class="outline-toggle"
    title="Search constructs (Ctrl/Cmd-K)"
    onclick={() => {
      open = !open;
      if (open) queueMicrotask(() => searchEl?.focus());
    }}
  >
    {open ? '▾' : '▸'} Outline
    <kbd class="kbd">⌘K</kbd>
  </button>
  {#if open}
    <div class="outline-panel">
      <input
        class="outline-search"
        type="search"
        placeholder="Search constructs…"
        bind:value={q}
        bind:this={searchEl}
      />
      <ul class="outline-list">
        {#each items as n}
          <li>
            <button type="button" class="outline-item" onclick={() => jump(n)}>
              <span class="sk">{n.metadata.subkind || n.kind}</span>
              <span class="nm">{n.name}</span>
            </button>
          </li>
        {/each}
        {#if items.length === 0}
          <li class="empty">No matches</li>
        {/if}
      </ul>
    </div>
  {/if}
</div>

<style>
  .outline {
    position: relative;
  }
  .outline-toggle {
    background: none;
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    color: var(--veil-text-dim);
    font-size: 11px;
    padding: 4px 8px;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .kbd {
    font-size: 9px;
    opacity: 0.6;
    border: 1px solid var(--veil-border);
    border-radius: 3px;
    padding: 0 3px;
  }
  .outline-panel {
    position: absolute;
    top: 100%;
    left: 0;
    margin-top: 4px;
    width: 280px;
    max-height: 360px;
    background: var(--veil-surface);
    border: 1px solid var(--veil-border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    z-index: 40;
    display: flex;
    flex-direction: column;
  }
  .outline-search {
    margin: 8px;
    padding: 6px 8px;
    border-radius: 4px;
    border: 1px solid var(--veil-border);
    background: var(--veil-surface-alt);
    color: var(--veil-text);
    font-size: 12px;
  }
  .outline-list {
    list-style: none;
    margin: 0;
    padding: 0 0 8px;
    overflow: auto;
  }
  .outline-item {
    width: 100%;
    text-align: left;
    border: none;
    background: transparent;
    color: var(--veil-text);
    padding: 6px 12px;
    cursor: pointer;
    display: flex;
    gap: 8px;
    font-size: 12px;
  }
  .outline-item:hover {
    background: var(--veil-accent-subtle);
  }
  .sk {
    color: var(--veil-text-dim);
    font-size: 10px;
    min-width: 72px;
  }
  .nm {
    font-family: 'JetBrains Mono', monospace;
  }
  .empty {
    padding: 8px 12px;
    font-size: 11px;
    color: var(--veil-text-faint);
    font-style: italic;
  }
</style>
