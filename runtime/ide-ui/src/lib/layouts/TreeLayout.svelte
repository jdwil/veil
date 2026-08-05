<script lang="ts">
  import { getNodeStyle, type IrGraph, type IrNode } from '$lib/types';
  import { selectedNodeId, irGraph, diagnostics, changedNodeIds } from '$lib/store';
  import { isCriticalNode } from '$lib/lenses';
  import type { ProjectResult } from '$lib/presentation';
  import type { PresentationModel } from '$lib/presentation';
  import { get } from 'svelte/store';

  let { projected, graph, presentationModel, onDrillDown }: {
    projected: ProjectResult;
    graph: IrGraph;
    presentationModel: PresentationModel | null;
    onDrillDown?: (node: IrNode) => void;
  } = $props();

  // Build the tree structure from nestEdges
  interface TreeNode {
    node: IrNode;
    children: TreeNode[];
    depth: number;
  }

  // Track which nodes are expanded (all expanded by default)
  let collapsed = $state<Set<number>>(new Set());

  let tree = $derived.by(() => {
    const childToParent = new Map<number, number>();
    for (const { child, parent } of projected.nestEdges) {
      childToParent.set(child, parent);
    }

    const nodeById = new Map<number, IrNode>();
    for (const n of graph.nodes) {
      nodeById.set(n.id, n);
    }

    // Find roots: nodes in projected.nodes that are NOT children in nestEdges
    const nestedIds = new Set(projected.nestEdges.map(e => e.child));
    const roots = projected.nodes.filter(n => !nestedIds.has(n.id));

    // Build children map
    const childrenOf = new Map<number, number[]>();
    for (const { child, parent } of projected.nestEdges) {
      if (!childrenOf.has(parent)) childrenOf.set(parent, []);
      childrenOf.get(parent)!.push(child);
    }

    function buildTree(node: IrNode, depth: number): TreeNode {
      const childIds = childrenOf.get(node.id) ?? [];
      const children = childIds
        .map(id => nodeById.get(id))
        .filter((n): n is IrNode => n != null)
        .sort((a, b) => a.span.start - b.span.start || a.name.localeCompare(b.name))
        .map(child => buildTree(child, depth + 1));
      return { node, children, depth };
    }

    return roots
      .sort((a, b) => a.span.start - b.span.start || a.name.localeCompare(b.name))
      .map(r => buildTree(r, 0));
  });

  // Orphans section
  let orphans = $derived.by(() => {
    if (!projected.orphanBucketLabel) return [];
    const orphanSet = new Set(projected.orphanIds);
    return graph.nodes
      .filter(n => orphanSet.has(n.id))
      .sort((a, b) => a.span.start - b.span.start);
  });

  function toggleCollapse(id: number) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    collapsed = next;
  }

  function selectNode(node: IrNode) {
    selectedNodeId.set(String(node.id));
  }

  function handleDoubleClick(node: IrNode) {
    if (onDrillDown) onDrillDown(node);
  }

  function isSelected(nodeId: number): boolean {
    return $selectedNodeId === String(nodeId);
  }

  function getNodeDiagnostics(nodeId: number) {
    return $diagnostics.filter(d => d.node_id === nodeId);
  }

  function hasError(nodeId: number): boolean {
    return getNodeDiagnostics(nodeId).some(d => d.severity === 'error');
  }

  function hasWarning(nodeId: number): boolean {
    return getNodeDiagnostics(nodeId).some(d => d.severity === 'warning');
  }

  function isChanged(nodeId: number): boolean {
    return $changedNodeIds.has(nodeId);
  }

  function getAnnotationPreview(node: IrNode): string[] {
    return node.metadata.annotations.filter(a => !a.startsWith('layer-provided')).slice(0, 3);
  }

  function getFieldCount(node: IrNode): number {
    const fields = node.metadata.properties.find(([k]) => k === 'fields')?.[1] ?? '';
    if (!fields) return 0;
    return fields.split(',').filter(Boolean).length;
  }

  function getMethodCount(node: IrNode): number {
    const methods = node.metadata.properties.find(([k]) => k === 'methods')?.[1] ?? '';
    if (!methods) return 0;
    return methods.split(';').filter(Boolean).length;
  }
</script>

<div class="tree-layout" role="tree" aria-label="Domain model tree">
  {#each tree as treeNode (treeNode.node.id)}
    {@render treeItem(treeNode)}
  {/each}

  {#if orphans.length > 0}
    <div class="tree-orphan-section">
      <div class="tree-orphan-header">
        <span class="tree-orphan-icon">📋</span>
        <span class="tree-orphan-label">{projected.orphanBucketLabel ?? 'Other'}</span>
        <span class="tree-orphan-count">{orphans.length}</span>
      </div>
      {#each orphans as orphan (orphan.id)}
        {@render leafItem(orphan, 1)}
      {/each}
    </div>
  {/if}
</div>

{#snippet treeItem(item: TreeNode)}
  {@const style = getNodeStyle(item.node.kind, item.node.metadata.subkind)}
  {@const hasChildren = item.children.length > 0}
  {@const isCollapsed = collapsed.has(item.node.id)}
  {@const selected = isSelected(item.node.id)}
  {@const changed = isChanged(item.node.id)}
  {@const errored = hasError(item.node.id)}
  {@const warned = hasWarning(item.node.id)}
  {@const critical = isCriticalNode(item.node, presentationModel, $diagnostics)}

  <div class="tree-item-group" role="treeitem" aria-expanded={hasChildren ? !isCollapsed : undefined} aria-selected={selected}>
    <div
      class="tree-row"
      class:selected
      class:changed
      class:errored
      class:warned
      class:critical
      style:padding-left="{12 + item.depth * 20}px"
      onclick={() => selectNode(item.node)}
      ondblclick={() => handleDoubleClick(item.node)}
      role="button"
      tabindex="0"
    >
      {#if hasChildren}
        <button
          class="tree-chevron"
          class:collapsed={isCollapsed}
          onclick={(e) => { e.stopPropagation(); toggleCollapse(item.node.id); }}
          aria-label={isCollapsed ? 'Expand' : 'Collapse'}
        >
          <svg width="12" height="12" viewBox="0 0 12 12">
            <path d="M4 2 L8 6 L4 10" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
          </svg>
        </button>
      {:else}
        <span class="tree-spacer"></span>
      {/if}

      <span class="tree-icon" title={style.label}>{style.icon}</span>
      <span class="tree-name">{item.node.name}</span>
      <span class="tree-kind">{item.node.metadata.subkind ?? item.node.kind}</span>

      {#if critical}
        <span class="tree-badge badge-critical" title="Critical">!</span>
      {/if}
      {#if errored}
        <span class="tree-badge badge-error" title="Has errors">⬤</span>
      {/if}
      {#if warned}
        <span class="tree-badge badge-warning" title="Has warnings">⬤</span>
      {/if}
      {#if changed}
        <span class="tree-badge badge-changed" title="Recently changed">●</span>
      {/if}

      {#if getFieldCount(item.node) > 0}
        <span class="tree-meta" title="Fields">{getFieldCount(item.node)}f</span>
      {/if}
      {#if getMethodCount(item.node) > 0}
        <span class="tree-meta" title="Methods">{getMethodCount(item.node)}m</span>
      {/if}
    </div>

    {#if hasChildren && !isCollapsed}
      <div class="tree-children" role="group">
        {#each item.children as child (child.node.id)}
          {@render treeItem(child)}
        {/each}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet leafItem(node: IrNode, depth: number)}
  {@const style = getNodeStyle(node.kind, node.metadata.subkind)}
  {@const selected = isSelected(node.id)}
  {@const changed = isChanged(node.id)}

  <button
    class="tree-row"
    class:selected
    class:changed
    style:padding-left="{12 + depth * 20}px"
    onclick={() => selectNode(node)}
    ondblclick={() => handleDoubleClick(node)}
    role="treeitem"
    aria-selected={selected}
  >
    <span class="tree-spacer"></span>
    <span class="tree-icon" title={style.label}>{style.icon}</span>
    <span class="tree-name">{node.name}</span>
    <span class="tree-kind">{node.metadata.subkind ?? node.kind}</span>
  </button>
{/snippet}

<style>
  .tree-layout {
    width: 100%;
    height: 100%;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 8px 0;
    background: var(--veil-bg);
    font-size: 13px;
    user-select: none;
  }

  .tree-item-group {
    display: contents;
  }

  .tree-row {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    height: 28px;
    padding-right: 12px;
    border: none;
    background: transparent;
    color: var(--veil-text);
    font: inherit;
    cursor: pointer;
    text-align: left;
    border-radius: 0;
    transition: background 0.1s;
  }

  .tree-row:hover {
    background: var(--veil-accent-hover);
  }

  .tree-row:focus-visible {
    outline: 1px solid var(--veil-accent);
    outline-offset: -1px;
  }

  .tree-row.selected {
    background: var(--veil-accent-hover);
    outline: 1px solid var(--veil-border);
  }

  .tree-row.changed {
    background: rgba(34, 197, 94, 0.05);
  }

  .tree-row.errored {
    background: rgba(239, 68, 68, 0.05);
  }

  .tree-chevron {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--veil-text-dim);
    cursor: pointer;
    flex-shrink: 0;
    transition: transform 0.15s ease;
  }

  .tree-chevron.collapsed {
    transform: rotate(0deg);
  }

  .tree-chevron:not(.collapsed) {
    transform: rotate(90deg);
  }

  .tree-spacer {
    width: 16px;
    flex-shrink: 0;
  }

  .tree-icon {
    flex-shrink: 0;
    width: 18px;
    text-align: center;
    font-size: 14px;
  }

  .tree-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
  }

  .tree-kind {
    color: var(--veil-text-faint);
    font-size: 11px;
    flex-shrink: 0;
  }

  .tree-badge {
    flex-shrink: 0;
    font-size: 10px;
    width: 14px;
    height: 14px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
  }

  .badge-critical {
    color: #f59e0b;
    font-weight: 700;
  }

  .badge-error {
    color: #ef4444;
    font-size: 8px;
  }

  .badge-warning {
    color: #f59e0b;
    font-size: 8px;
  }

  .badge-changed {
    color: #22c55e;
    font-size: 8px;
  }

  .tree-meta {
    color: var(--veil-text-faint);
    font-size: 10px;
    flex-shrink: 0;
    padding: 1px 4px;
    border-radius: 3px;
    background: var(--veil-accent-subtle);
  }

  .tree-children {
    display: block;
  }

  .tree-orphan-section {
    margin-top: 12px;
    border-top: 1px solid var(--veil-border);
    padding-top: 8px;
  }

  .tree-orphan-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    color: var(--veil-text-dim);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .tree-orphan-icon {
    font-size: 12px;
  }

  .tree-orphan-label {
    flex: 1;
  }

  .tree-orphan-count {
    padding: 1px 5px;
    border-radius: 8px;
    background: var(--veil-accent-subtle);
    font-size: 10px;
  }
</style>
