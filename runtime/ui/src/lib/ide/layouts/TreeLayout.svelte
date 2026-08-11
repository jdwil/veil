<script lang="ts">
  import { getNodeStyle, paletteStylesVersion, type IrGraph, type IrNode } from '$lib/ide/types';
  import { onMount } from 'svelte';
  import {
    selectedNodeId,
    diagnostics,
    changedNodeIds,
    focusDiagnostic,
    type Diagnostic,
  } from '$lib/ide/store';
  import { nodeHasReviewLens, nodeHasHealthIssue } from '$lib/ide/lenses';
  import type { ProjectResult, PresentationModel } from '$lib/ide/presentation';
  import { canDrillInto, irChildren } from '$lib/ide/presentation';
  import {
    outlineCollapseScope,
    loadCollapsedKeys,
    saveCollapsedKeys,
  } from '$lib/ide/outlineLayout';

  let { projected, graph, presentationModel, onDrillDown }: {
    projected: ProjectResult;
    graph: IrGraph;
    presentationModel: PresentationModel | null;
    onDrillDown?: (node: IrNode) => void;
  } = $props();

  /**
   * Outline rows: real IR nodes, or synthetic kind folders (visual only —
   * not in IR). Kind folders cut clutter under Groups / multi-kind parents.
   *
   * `collapseKey` is a stable path string for localStorage (survives IR id churn).
   */
  type TreeNode =
    | {
        type: 'ir';
        node: IrNode;
        collapseKey: string;
        children: TreeNode[];
        depth: number;
      }
    | {
        type: 'kind';
        collapseKey: string;
        key: string;
        label: string;
        icon: string;
        children: TreeNode[];
        depth: number;
      };

  /**
   * Soft preference order for kind folders. Keys are construct type labels
   * from IR (`subkind ?? kind`) — unknown keys still appear (alpha). This is
   * not DDD logic: core kinds like InterfaceMethod / Flow / TypeDef work for
   * any layer; palette subkinds just sort nicer when present.
   */
  const KIND_ORDER = [
    'Module',
    'Group',
    'TypeDef',
    'Interface',
    'Implementation',
    'Flow',
    'InterfaceMethod',
    // Common layer subkinds (order only; not required for outline correctness)
    'Aggregate',
    'Entity',
    'ValueObject',
    'enum',
    'Event',
    'Command',
    'Query',
    'DomainService',
    'ApplicationService',
    'Repository',
    'Port',
    'Adapter',
    'Handler',
    'Service',
    'Orchestrator',
    'Saga',
  ];

  /** Collapsed row keys (path-based). Empty = everything expanded. */
  let collapseScope = $state(
    typeof window !== 'undefined' ? outlineCollapseScope() : 'veil.outline.collapsed:default'
  );
  let collapsed = $state<Set<string>>(
    typeof window !== 'undefined'
      ? loadCollapsedKeys(outlineCollapseScope())
      : new Set()
  );

  // Re-read if the SPA navigates to another project without full remount.
  onMount(() => {
    const scope = outlineCollapseScope();
    collapseScope = scope;
    collapsed = loadCollapsedKeys(scope);
  });

  function kindKey(n: IrNode): string {
    return n.metadata.subkind ?? n.kind;
  }

  function pluralLabel(label: string): string {
    const t = label.trim();
    if (!t) return 'Items';
    if (/s$/i.test(t) && !/ss$/i.test(t)) return t;
    if (/[^aeiou]y$/i.test(t)) return t.slice(0, -1) + 'ies';
    return t + 's';
  }

  /** Stable path segment for a node (name + construct type, not IR id). */
  function nodeSegment(n: IrNode): string {
    if (n.kind === 'Group') return `g:${n.name}`;
    if (n.kind === 'Solution') return `sol:${n.name}`;
    const sk = n.metadata.subkind ?? n.kind;
    return `${sk}:${n.name}`;
  }

  function nodePathKey(node: IrNode, byId: Map<number, IrNode>): string {
    const parts: string[] = [];
    let cur: IrNode | undefined = node;
    const seen = new Set<number>();
    while (cur && !seen.has(cur.id)) {
      seen.add(cur.id);
      if (cur.kind !== 'Solution') {
        parts.unshift(nodeSegment(cur));
      }
      const pid = cur.metadata.parent;
      cur = pid != null ? byId.get(pid) : undefined;
    }
    return parts.join('/') || nodeSegment(node);
  }

  function sortKindKeys(keys: string[]): string[] {
    return [...keys].sort((a, b) => {
      const ai = KIND_ORDER.indexOf(a);
      const bi = KIND_ORDER.indexOf(b);
      return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi) || a.localeCompare(b);
    });
  }

  function sortIr(a: IrNode, b: IrNode): number {
    return a.span.start - b.span.start || a.name.localeCompare(b.name);
  }

  /**
   * Insert kind folders when:
   * - parent is an architectural Group, or
   * - parent is a type/interface/impl host with methods, or
   * - siblings span 2+ distinct construct kinds
   * Groups as children stay flat (they are already layer folders).
   */
  function shouldKindFolder(parent: IrNode | null, kids: IrNode[]): boolean {
    if (kids.length === 0) return false;
    if (kids.every((k) => k.kind === 'Group')) return false;
    if (parent?.kind === 'Group') return true;
    // Core hosts that commonly own methods — always bucket so Methods is visible
    if (
      parent &&
      (parent.kind === 'TypeDef' ||
        parent.kind === 'Interface' ||
        parent.kind === 'Implementation') &&
      kids.some((k) => k.kind === 'InterfaceMethod')
    ) {
      return true;
    }
    return new Set(kids.map(kindKey)).size >= 2;
  }

  let tree = $derived.by(() => {
    // Depend on palette registration so Aggregate/VO folders re-label after
    // secondary /api/palette load (otherwise stuck on TypeDef "Types").
    void $paletteStylesVersion;

    const nodeById = new Map<number, IrNode>();
    for (const n of graph.nodes) nodeById.set(n.id, n);

    const nestedIds = new Set(projected.nestEdges.map((e) => e.child));
    const roots = projected.nodes.filter((n) => !nestedIds.has(n.id));

    const childrenOf = new Map<number, number[]>();
    for (const { child, parent } of projected.nestEdges) {
      if (!childrenOf.has(parent)) childrenOf.set(parent, []);
      childrenOf.get(parent)!.push(child);
    }

    /**
     * Children for outline expansion: prefer nestEdges from projection, but
     * always union direct InterfaceMethod IR children so methods never vanish
     * if a projection path omits them.
     */
    function childNodesOf(parentId: number): IrNode[] {
      const fromNest = (childrenOf.get(parentId) ?? [])
        .map((id) => nodeById.get(id))
        .filter((n): n is IrNode => n != null);
      const seen = new Set(fromNest.map((n) => n.id));
      // Core-kind safety net: class/port/adapter methods on the type host
      for (const d of irChildren(graph, parentId)) {
        if (d.kind !== 'InterfaceMethod') continue;
        if (seen.has(d.id)) continue;
        seen.add(d.id);
        fromNest.push(d);
      }
      return fromNest.sort(sortIr);
    }

    function buildIr(node: IrNode, depth: number): TreeNode {
      const kids = childNodesOf(node.id);
      const collapseKey = nodePathKey(node, nodeById);
      const children = wrapByKind(node, collapseKey, kids, depth + 1);
      return { type: 'ir', node, collapseKey, children, depth };
    }

    function wrapByKind(
      parent: IrNode | null,
      parentPath: string,
      kids: IrNode[],
      depth: number
    ): TreeNode[] {
      if (!shouldKindFolder(parent, kids)) {
        return kids.map((k) => buildIr(k, depth));
      }

      const buckets = new Map<string, IrNode[]>();
      for (const k of kids) {
        // Bucket methods by core kind so the folder is "Methods", not a blank subkind
        const key =
          k.kind === 'InterfaceMethod' ? 'InterfaceMethod' : kindKey(k);
        if (!buckets.has(key)) buckets.set(key, []);
        buckets.get(key)!.push(k);
      }

      const base = parentPath || '@root';
      return sortKindKeys([...buckets.keys()]).map((key) => {
        const members = buckets.get(key)!;
        const sample = members[0];
        const style =
          key === 'InterfaceMethod'
            ? getNodeStyle('InterfaceMethod', null)
            : getNodeStyle(sample.kind, sample.metadata.subkind);
        const collapseKey = `${base}/@kind:${key}`;
        return {
          type: 'kind' as const,
          collapseKey,
          key,
          label: pluralLabel(style.label),
          icon: style.icon,
          children: members.map((m) => buildIr(m, depth + 1)),
          depth,
        };
      });
    }

    return wrapByKind(null, '@root', roots.sort(sortIr), 0);
  });

  let orphans = $derived.by(() => {
    if (!projected.orphanBucketLabel) return [];
    const orphanSet = new Set(projected.orphanIds);
    return graph.nodes.filter((n) => orphanSet.has(n.id)).sort(sortIr);
  });

  function toggleCollapse(key: string) {
    const next = new Set(collapsed);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    collapsed = next;
    // Persist immediately so refresh restores (scope = project only).
    saveCollapsedKeys(collapseScope || outlineCollapseScope(), next);
  }

  function selectNode(node: IrNode) {
    selectedNodeId.set(String(node.id));
  }

  function handleDoubleClick(node: IrNode, collapseKey: string) {
    if (canDrillInto(graph, node)) {
      onDrillDown?.(node);
      return;
    }
    const hasKids = projected.nestEdges.some((e) => e.parent === node.id);
    if (hasKids) toggleCollapse(collapseKey);
  }

  function isGroupFolder(node: IrNode): boolean {
    return node.kind === 'Group';
  }

  function rowTitle(node: IrNode): string {
    if (canDrillInto(graph, node)) return 'Double-click to open flow graph';
    if (projected.nestEdges.some((e) => e.parent === node.id)) {
      return isGroupFolder(node)
        ? 'Architectural layer — collapse/expand'
        : 'Expand/collapse in outline';
    }
    return '';
  }

  function isSelected(nodeId: number): boolean {
    return $selectedNodeId === String(nodeId);
  }

  function diagMatchesNode(d: Diagnostic, node: IrNode): boolean {
    if (d.node_id != null && d.node_id === node.id) return true;
    if (d.node_name != null && d.node_name === node.name) return true;
    return false;
  }

  function getNodeDiagnostics(node: IrNode): Diagnostic[] {
    return $diagnostics.filter((d) => diagMatchesNode(d, node));
  }

  function sev(d: Diagnostic): string {
    return (d.severity ?? '').toLowerCase();
  }

  function hasError(node: IrNode): boolean {
    return getNodeDiagnostics(node).some((d) => sev(d) === 'error');
  }

  function hasWarning(node: IrNode): boolean {
    return getNodeDiagnostics(node).some(
      (d) => sev(d) === 'warning' || sev(d) === 'guidance'
    );
  }

  function diagnosticTitle(node: IrNode): string {
    const diags = getNodeDiagnostics(node);
    if (diags.length === 0) {
      if (nodeHasReviewLens(node, presentationModel)) {
        return 'Review focus (layer lens) — architecturally important, not a check failure';
      }
      return '';
    }
    return diags
      .map((d) => {
        const code = d.code ? `[${d.code}] ` : '';
        const hint = d.hint ? `\n  ↳ ${d.hint}` : '';
        return `${d.severity}: ${code}${d.message}${hint}`;
      })
      .join('\n');
  }

  function onBadgeClick(e: MouseEvent, node: IrNode) {
    e.stopPropagation();
    selectNode(node);
    const diags = getNodeDiagnostics(node);
    if (diags[0]) {
      focusDiagnostic(diags[0]);
    }
    // Review-lens only: select node; do not fabricate a Warning diagnostic
  }

  function isChanged(nodeId: number): boolean {
    return $changedNodeIds.has(nodeId);
  }

  function getFieldCount(node: IrNode): number {
    const fields = node.metadata.properties.find(([k]) => k === 'fields')?.[1] ?? '';
    if (!fields) return 0;
    return fields.split(',').filter(Boolean).length;
  }

  function getMethodCount(node: IrNode): number {
    // Prefer live IR children (class/port methods) over summary property strings
    const fromIr = graph.nodes.filter(
      (n) => n.metadata.parent === node.id && n.kind === 'InterfaceMethod'
    ).length;
    if (fromIr > 0) return fromIr;
    const methods = node.metadata.properties.find(([k]) => k === 'methods')?.[1] ?? '';
    if (!methods) return 0;
    return methods.split(';').filter(Boolean).length;
  }

  function rowKey(item: TreeNode): string {
    return item.collapseKey;
  }
</script>

<div class="tree-layout" role="tree" aria-label="Domain model tree">
  {#each tree as treeNode (rowKey(treeNode))}
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
  {#if item.type === 'kind'}
    {@const isCollapsed = collapsed.has(item.collapseKey)}
    <div
      class="tree-item-group"
      role="treeitem"
      aria-expanded={!isCollapsed}
      aria-selected={false}
    >
      <div
        class="tree-row kind-folder"
        style:padding-left="{12 + item.depth * 20}px"
        onclick={() => toggleCollapse(item.collapseKey)}
        ondblclick={() => toggleCollapse(item.collapseKey)}
        title="Construct kind — collapse/expand"
        role="button"
        tabindex="0"
        onkeydown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            toggleCollapse(item.collapseKey);
          }
        }}
      >
        <button
          class="tree-chevron"
          class:collapsed={isCollapsed}
          onclick={(e) => {
            e.stopPropagation();
            toggleCollapse(item.collapseKey);
          }}
          aria-label={isCollapsed ? 'Expand' : 'Collapse'}
        >
          <svg width="12" height="12" viewBox="0 0 12 12">
            <path
              d="M4 2 L8 6 L4 10"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
            />
          </svg>
        </button>
        <span class="tree-icon" title={item.label}>{item.icon}</span>
        <span class="tree-name kind-name">{item.label}</span>
        <span class="tree-kind">kind</span>
        <span class="tree-meta" title="Count">{item.children.length}</span>
      </div>
      {#if !isCollapsed}
        <div class="tree-children" role="group">
          {#each item.children as child (rowKey(child))}
            {@render treeItem(child)}
          {/each}
        </div>
      {/if}
    </div>
  {:else}
    {@const node = item.node}
    {@const style = getNodeStyle(node.kind, node.metadata.subkind)}
    {@const hasChildren = item.children.length > 0}
    {@const isCollapsed = collapsed.has(item.collapseKey)}
    {@const selected = isSelected(node.id)}
    {@const changed = isChanged(node.id)}
    {@const nodeDiags = getNodeDiagnostics(node)}
    {@const errored = hasError(node)}
    {@const warned = hasWarning(node)}
    {@const reviewLens = nodeHasReviewLens(node, presentationModel)}
    {@const healthIssue = nodeHasHealthIssue(node, $diagnostics)}
    {@const diagTitle = diagnosticTitle(node)}

    <div
      class="tree-item-group"
      role="treeitem"
      aria-expanded={hasChildren ? !isCollapsed : undefined}
      aria-selected={selected}
    >
      <div
        class="tree-row"
        class:selected
        class:changed
        class:errored
        class:warned
        class:review-lens={reviewLens && !errored && !warned}
        class:health-issue={healthIssue}
        class:group-folder={isGroupFolder(node)}
        class:drillable={canDrillInto(graph, node)}
        style:padding-left="{12 + item.depth * 20}px"
        onclick={() => selectNode(node)}
        ondblclick={() => handleDoubleClick(node, item.collapseKey)}
        title={diagTitle || rowTitle(node)}
        role="button"
        tabindex="0"
      >
        {#if hasChildren}
          <button
            class="tree-chevron"
            class:collapsed={isCollapsed}
            onclick={(e) => {
              e.stopPropagation();
              toggleCollapse(item.collapseKey);
            }}
            aria-label={isCollapsed ? 'Expand' : 'Collapse'}
          >
            <svg width="12" height="12" viewBox="0 0 12 12">
              <path
                d="M4 2 L8 6 L4 10"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
              />
            </svg>
          </button>
        {:else}
          <span class="tree-spacer"></span>
        {/if}

        <span class="tree-icon" title={style.label}
          >{isGroupFolder(node) ? (isCollapsed ? '📁' : '📂') : style.icon}</span
        >
        <span class="tree-name" class:group-name={isGroupFolder(node)}>{node.name}</span>
        {#if !isGroupFolder(node)}
          <span class="tree-kind">{node.metadata.subkind ?? node.kind}</span>
        {:else}
          <span class="tree-kind">layer</span>
        {/if}
        {#if canDrillInto(graph, node)}
          <span class="tree-drill-hint" title="Open flow graph">⤵</span>
        {/if}

        {#if errored || warned}
          <button
            type="button"
            class="tree-badge"
            class:badge-error={errored}
            class:badge-warning={warned && !errored}
            title={diagTitle || 'Diagnostics'}
            aria-label={diagTitle || 'Show diagnostics'}
            onclick={(e) => onBadgeClick(e, node)}
          >
            {errored ? '⬤' : '⬤'}
            {#if nodeDiags.length > 1}
              <span class="badge-count">{nodeDiags.length}</span>
            {/if}
          </button>
        {:else if reviewLens || healthIssue}
          <span
            class="tree-badge"
            class:badge-review-lens={reviewLens && !healthIssue}
            class:badge-critical={healthIssue}
            title={healthIssue
              ? 'Error or escape-hatch on this construct'
              : 'Review focus (layer lens) — not a check failure'}
          >
            {healthIssue ? '!' : '🔍'}
          </span>
        {/if}
        {#if changed}
          <span class="tree-badge badge-changed" title="Recently changed">●</span>
        {/if}

        {#if getFieldCount(node) > 0}
          <span class="tree-meta" title="Fields">{getFieldCount(node)}f</span>
        {/if}
        {#if getMethodCount(node) > 0}
          <span class="tree-meta" title="Methods">{getMethodCount(node)}m</span>
        {/if}
      </div>

      {#if hasChildren && !isCollapsed}
        <div class="tree-children" role="group">
          {#each item.children as child (rowKey(child))}
            {@render treeItem(child)}
          {/each}
        </div>
      {/if}
    </div>
  {/if}
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
    ondblclick={() => handleDoubleClick(node, nodeSegment(node))}
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

  .tree-name.group-name {
    font-weight: 600;
    text-transform: lowercase;
    letter-spacing: 0.02em;
    color: var(--veil-text-secondary, var(--veil-text-dim));
  }

  .tree-name.kind-name {
    font-weight: 600;
    color: var(--veil-text-secondary, var(--veil-text-dim));
  }

  .tree-row.group-folder {
    opacity: 0.95;
  }

  .tree-row.group-folder .tree-kind,
  .tree-row.kind-folder .tree-kind {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .tree-row.kind-folder {
    height: 26px;
    opacity: 0.92;
  }

  .tree-drill-hint {
    flex-shrink: 0;
    font-size: 11px;
    color: var(--veil-text-dim);
    opacity: 0.7;
  }

  .tree-row.drillable:hover .tree-drill-hint {
    opacity: 1;
    color: var(--veil-accent, var(--veil-text));
  }

  .tree-kind {
    color: var(--veil-text-faint);
    font-size: 11px;
    flex-shrink: 0;
  }

  .tree-badge {
    flex-shrink: 0;
    font-size: 10px;
    min-width: 14px;
    height: 16px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 1px;
    border-radius: 8px;
    border: none;
    background: transparent;
    padding: 0 3px;
    cursor: pointer;
    color: inherit;
  }

  .tree-badge:hover {
    background: var(--veil-accent-hover, rgba(115, 115, 115, 0.25));
  }

  .badge-critical {
    color: #f59e0b;
    font-weight: 700;
  }

  .badge-review-lens {
    color: #38bdf8;
    font-size: 11px;
    opacity: 0.95;
  }

  .tree-row.review-lens {
    /* Soft blue wash — not warning amber */
    box-shadow: inset 2px 0 0 rgba(56, 189, 248, 0.45);
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

  .badge-count {
    font-size: 9px;
    font-weight: 700;
    line-height: 1;
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
    margin-top: 8px;
    border-top: 1px solid var(--veil-border);
    padding-top: 4px;
  }

  .tree-orphan-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    color: var(--veil-text-dim);
    font-size: 12px;
    font-weight: 600;
  }
</style>
