<script lang="ts">
  /**
   * UX-020: Primary VEIL source / critical-body review pane.
   * Shows full package source or a span-focused excerpt for the selection.
   */
  import { veilSource, irGraph, selectedNodeId } from '$lib/store';
  import { get } from 'svelte/store';

  interface Props {
    /** When true, chrome is provided by ReviewDock (no own collapse header). */
    embedded?: boolean;
  }
  let { embedded = false }: Props = $props();

  let visible = $state(true);
  let focusSelection = $state(true);

  function extractForSelection(source: string): { title: string; text: string } {
    const graph = get(irGraph);
    const sid = get(selectedNodeId);
    if (!focusSelection || !graph || !sid) {
      return { title: 'Package source', text: source };
    }
    const node = graph.nodes.find((n) => String(n.id) === sid);
    if (!node || node.span.start === node.span.end) {
      return { title: 'Package source', text: source };
    }
    let start = node.span.start;
    let end = Math.min(node.span.end, source.length);
    while (start > 0 && source[start - 1] !== '\n') start--;
    while (end < source.length && source[end] !== '\n') end++;
    const text = source.slice(start, end).trimEnd();
    const kind = node.metadata.subkind || node.kind;
    return {
      title: `${kind} ${node.name}`,
      text: text || source,
    };
  }

  let excerpt = $derived(extractForSelection($veilSource || ''));
</script>

<div class="veil-source" class:embedded>
  {#if !embedded}
    <div class="vs-header">
      <button type="button" class="toggle" onclick={() => (visible = !visible)}>
        {visible ? '▾' : '▸'} VEIL source
      </button>
      {#if visible}
        <label class="focus-toggle">
          <input type="checkbox" bind:checked={focusSelection} />
          Selection focus
        </label>
        <span class="vs-title">{excerpt.title}</span>
      {/if}
    </div>
  {:else}
    <div class="vs-header embedded-header">
      <label class="focus-toggle">
        <input type="checkbox" bind:checked={focusSelection} />
        Selection focus
      </label>
      <span class="vs-title">{excerpt.title}</span>
    </div>
  {/if}
  {#if visible || embedded}
    <pre class="vs-body"><code>{$veilSource ? excerpt.text : 'No source loaded.'}</code></pre>
  {/if}
</div>

<style>
  .veil-source {
    border-top: 1px solid var(--veil-border);
    background: var(--veil-surface);
    max-height: 40vh;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .veil-source.embedded {
    border-top: none;
    max-height: none;
    height: 100%;
    flex: 1;
  }
  .vs-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    border-bottom: 1px solid var(--veil-border);
    flex-shrink: 0;
  }
  .embedded-header {
    padding: 4px 12px;
  }
  .toggle {
    background: none;
    border: none;
    color: var(--veil-text);
    font-weight: 700;
    font-size: 11px;
    cursor: pointer;
    letter-spacing: 0.03em;
  }
  .focus-toggle {
    font-size: 10px;
    color: var(--veil-text-dim);
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .vs-title {
    font-size: 10px;
    color: var(--veil-text-secondary);
    margin-left: auto;
    font-family: 'JetBrains Mono', monospace;
  }
  .vs-body {
    margin: 0;
    padding: 10px 14px;
    overflow: auto;
    flex: 1;
    font-size: 11px;
    line-height: 1.45;
    font-family: 'JetBrains Mono', ui-monospace, monospace;
    color: var(--veil-text);
    white-space: pre;
  }
  .vs-body code {
    font: inherit;
  }
</style>
