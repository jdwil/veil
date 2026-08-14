<script lang="ts">
  import { getNodeStyle, paletteStylesVersion, type IrGraph, type IrNode } from '$lib/ide/types';
  import { selectedNodeId, diagnostics, changedNodeIds } from '$lib/ide/store';
  import { nodeHasReviewLens, nodeHasHealthIssue } from '$lib/ide/lenses';
  import type { ProjectResult, PresentationModel } from '$lib/ide/presentation';

  let { projected, graph, presentationModel, onDrillDown }: {
    projected: ProjectResult;
    graph: IrGraph;
    presentationModel: PresentationModel | null;
    onDrillDown?: (node: IrNode) => void;
  } = $props();

  // Group nodes by construct type (subkind)
  interface TypeGroup {
    type: string;
    icon: string;
    color: string;
    label: string;
    nodes: IrNode[];
  }

  let groups = $derived.by((): TypeGroup[] => {
    void $paletteStylesVersion;
    const groupMap = new Map<string, IrNode[]>();
    for (const node of projected.nodes) {
      const type = node.metadata.subkind ?? node.kind;
      if (!groupMap.has(type)) groupMap.set(type, []);
      groupMap.get(type)!.push(node);
    }

    const result: TypeGroup[] = [];
    for (const [type, nodes] of groupMap) {
      const style = getNodeStyle(nodes[0].kind, nodes[0].metadata.subkind);
      result.push({
        type,
        icon: style.icon,
        color: style.color,
        label: style.label,
        nodes: nodes.sort((a, b) => a.span.start - b.span.start || a.name.localeCompare(b.name)),
      });
    }

    // Sort groups: containers first, then by count descending
    return result.sort((a, b) => b.nodes.length - a.nodes.length);
  });

  function selectNode(node: IrNode) {
    selectedNodeId.set(String(node.id));
  }

  function handleDoubleClick(node: IrNode) {
    if (onDrillDown) onDrillDown(node);
  }

  function isSelected(nodeId: number): boolean {
    return $selectedNodeId === String(nodeId);
  }

  function hasError(nodeId: number): boolean {
    return $diagnostics.some(d => d.node_id === nodeId && d.severity === 'error');
  }

  function hasWarning(nodeId: number): boolean {
    return $diagnostics.some(d => d.node_id === nodeId && d.severity === 'warning');
  }

  function isChanged(nodeId: number): boolean {
    return $changedNodeIds.has(nodeId);
  }

  function getFieldsPreview(node: IrNode): string {
    return node.metadata.properties.find(([k]) => k === 'fields')?.[1] ?? '';
  }

  function getMethodsPreview(node: IrNode): string {
    const methods = node.metadata.properties.find(([k]) => k === 'methods')?.[1] ?? '';
    if (!methods) return '';
    return methods.split(';').filter(Boolean).map(m => m.trim().split('(')[0]).join(', ');
  }
</script>

<div class="flat-layout" role="list" aria-label="Constructs by type">
  {#each groups as group (group.type)}
    <section class="flat-group" aria-label="{group.label} ({group.nodes.length})">
      <header class="flat-group-header">
        <span class="flat-group-icon">{group.icon}</span>
        <span class="flat-group-label">{group.label}</span>
        <span class="flat-group-count">{group.nodes.length}</span>
      </header>

      <div class="flat-group-items">
        {#each group.nodes as node (node.id)}
          {@const selected = isSelected(node.id)}
          {@const changed = isChanged(node.id)}
          {@const errored = hasError(node.id)}
          {@const warned = hasWarning(node.id)}
          {@const fields = getFieldsPreview(node)}
          {@const methods = getMethodsPreview(node)}
          {@const reviewLens = nodeHasReviewLens(node, presentationModel)}
          {@const healthIssue = nodeHasHealthIssue(node, $diagnostics)}

          <button
            class="flat-item"
            class:selected
            class:changed
            class:errored
            class:warned
            onclick={() => selectNode(node)}
            ondblclick={() => handleDoubleClick(node)}
            aria-pressed={selected}
          >
            <div class="flat-item-main">
              <span class="flat-item-name">{node.name}</span>
              {#if healthIssue}
                <span class="flat-badge badge-critical" title="Error or escape-hatch">!</span>
              {:else if reviewLens}
                <span class="flat-badge badge-review-lens" title="Review focus (layer lens)">🔍</span>
              {/if}
              {#if errored}
                <span class="flat-badge badge-error" title="Error">⬤</span>
              {/if}
              {#if warned}
                <span class="flat-badge badge-warning" title="Warning">⬤</span>
              {/if}
              {#if changed}
                <span class="flat-badge badge-changed" title="Changed">●</span>
              {/if}
            </div>
            {#if fields || methods}
              <div class="flat-item-meta">
                {#if fields}
                  <span class="flat-item-fields" title={fields}>
                    {fields.split(',').filter(Boolean).length} fields
                  </span>
                {/if}
                {#if methods}
                  <span class="flat-item-methods" title={methods}>{methods}</span>
                {/if}
              </div>
            {/if}
            {#if node.metadata.annotations.length > 0}
              <div class="flat-item-annotations">
                {#each node.metadata.annotations.filter(a => !a.startsWith('layer-provided')).slice(0, 4) as ann}
                  <span class="flat-annotation">@{ann}</span>
                {/each}
              </div>
            {/if}
          </button>
        {/each}
      </div>
    </section>
  {/each}

  {#if groups.length === 0}
    <div class="flat-empty">
      <p>No constructs in this view.</p>
    </div>
  {/if}
</div>

<style>
  .flat-layout {
    width: 100%;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 12px;
    background: var(--veil-bg);
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .flat-group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .flat-group-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    color: var(--veil-text-dim);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    border-bottom: 1px solid var(--veil-border);
    margin-bottom: 4px;
  }

  .flat-group-icon {
    font-size: 13px;
  }

  .flat-group-label {
    flex: 1;
  }

  .flat-group-count {
    padding: 1px 6px;
    border-radius: 8px;
    background: var(--veil-accent-subtle);
    font-size: 10px;
    color: var(--veil-text-faint);
  }

  .flat-group-items {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .flat-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    padding: 8px 12px;
    border: none;
    background: transparent;
    color: var(--veil-text);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
    text-align: left;
    border-radius: 4px;
    transition: background 0.1s;
  }

  .flat-item:hover {
    background: var(--veil-accent-hover);
  }

  .flat-item.selected {
    background: var(--veil-accent-hover);
    outline: 1px solid var(--veil-border);
  }

  .flat-item.changed {
    border-left: 2px solid #22c55e;
  }

  .flat-item.errored {
    border-left: 2px solid #ef4444;
  }

  .flat-item.warned {
    border-left: 2px solid #f59e0b;
  }

  .flat-item-main {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .flat-item-name {
    font-weight: 500;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .flat-badge {
    flex-shrink: 0;
    font-size: 10px;
  }

  .badge-critical { color: #f59e0b; font-weight: 700; }
  .badge-review-lens { color: #38bdf8; font-size: 11px; }
  .badge-error { color: #ef4444; font-size: 8px; }
  .badge-warning { color: #f59e0b; font-size: 8px; }
  .badge-changed { color: #22c55e; font-size: 8px; }

  .flat-item-meta {
    display: flex;
    gap: 8px;
    padding-left: 2px;
    color: var(--veil-text-faint);
    font-size: 11px;
  }

  .flat-item-fields {
    color: var(--veil-text-dim);
  }

  .flat-item-methods {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 200px;
    font-family: var(--font-mono);
    font-size: 10px;
  }

  .flat-item-annotations {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
    padding-left: 2px;
  }

  .flat-annotation {
    font-size: 10px;
    padding: 1px 4px;
    border-radius: 3px;
    background: var(--veil-accent-subtle);
    color: var(--veil-text-dim);
    font-family: var(--font-mono);
  }

  .flat-empty {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100px;
    color: var(--veil-text-faint);
    font-size: 13px;
  }
</style>
