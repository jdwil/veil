<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import {
    SvelteFlow,
    Controls,
    Background,
    BackgroundVariant,
    MiniMap,
    type Node,
    type Edge,
    type NodeTypes,
  } from '@xyflow/svelte';
  import '@xyflow/svelte/dist/style.css';

  import VeilNode from '$lib/VeilNode.svelte';
  import Palette from '$lib/Palette.svelte';
  import PropertyEditor from '$lib/PropertyEditor.svelte';
  import CodePreview from '$lib/CodePreview.svelte';
  import { layoutNodes } from '$lib/layout';
  import {
    irGraph,
    currentParent,
    breadcrumbs,
    loading,
    error,
    fetchIr,
    drillDown,
    navigateTo,
    getChildren,
    selectedNodeId,
    paletteConfig,
    saveEdits,
  } from '$lib/store';
  import { NODE_STYLES, type IrNode, type IrGraph, type NodeKind } from '$lib/types';

  const nodeTypes: NodeTypes = {
    veil: VeilNode as any,
  };

  let nodes = $state.raw<Node[]>([]);
  let edges = $state.raw<Edge[]>([]);
  let nextNodeId = $state(1000);
  let tabs = $state<string[]>([]);
  let activeTab = $state<string | null>(null);
  let showLayerProvided = $state(false);

  // Derive the current context kind for palette filtering
  let currentContextKind = $derived.by(() => {
    const graph = $irGraph;
    const parent = $currentParent;
    if (!graph || !parent) return 'Solution';
    const parentNode = graph.nodes.find(n => n.id === parent);
    // Use subkind (layer name like "Context", "Orchestrator") for palette
    // filtering, falling back to the core kind.
    return parentNode?.metadata.subkind ?? parentNode?.kind ?? 'Solution';
  });

  // Derive scope variables from the current flow's Inputs node + parent chain
  let scopeVars = $derived.by(() => {
    const graph = $irGraph;
    const parent = $currentParent;
    if (!graph || !parent) return [] as string[];
    const vars: string[] = [];

    // Walk up the parent chain looking for Inputs nodes
    let current: number | null = parent;
    while (current !== null) {
      const children = graph.nodes.filter(n => n.metadata.parent === current);
      const inputsNode = children.find(n => n.kind === 'Inputs');
      if (inputsNode) {
        const params = inputsNode.metadata.properties.find(([k]) => k === 'params');
        if (params) {
          const paramList = params[1].split(', ').map(p => p.trim());
          vars.push(...paramList);
        }
      }
      // Also find assigns at this level (variables created by steps)
      for (const child of children) {
        if (child.kind === 'Action' && child.metadata.subkind === 'assign') {
          const assignName = child.name.split(' = ')[0];
          if (assignName && !vars.includes(assignName)) {
            vars.push(assignName);
          }
        }
      }
      const parentNode = graph.nodes.find(n => n.id === current);
      current = parentNode?.metadata.parent ?? null;
    }
    return vars;
  });

  // Get the currently selected node for property editing
  // Action-level nodes get specialized editors (Task 6 of this refactor)

  let selectedNode = $derived.by(() => {
    const id = $selectedNodeId;
    if (!id) return null;
    return nodes.find(n => n.id === id) ?? null;
  });

  function updateNodeData(id: string, data: any) {
    nodes = nodes.map(n => n.id === id ? { ...n, data } : n);
  }

  function handleDrop(event: DragEvent) {
    event.preventDefault();
    if (!event.dataTransfer) return;

    const data = event.dataTransfer.getData('application/veil-node');
    if (!data) return;

    const item = JSON.parse(data) as { kind: NodeKind; label: string; icon: string; name?: string };

    // Create new node at drop position (convert screen coords to flow coords)
    const id = String(nextNodeId++);
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();

    // Get the viewport transform from the xyflow DOM to convert screen → flow coords
    const viewportEl = (event.currentTarget as HTMLElement).querySelector('.svelte-flow__viewport');
    let position = { x: event.clientX - rect.left, y: event.clientY - rect.top };
    if (viewportEl) {
      const transform = window.getComputedStyle(viewportEl).transform;
      // Parse matrix(a, b, c, d, tx, ty) — a=scale, tx/ty=translate
      const match = transform.match(/matrix\(([^)]+)\)/);
      if (match) {
        const parts = match[1].split(',').map(Number);
        const scale = parts[0];
        const tx = parts[4];
        const ty = parts[5];
        position = {
          x: (event.clientX - rect.left - tx) / scale,
          y: (event.clientY - rect.top - ty) / scale,
        };
      }
    }

    const newNode: Node = {
      id,
      type: 'veil',
      position,
      data: {
        label: `New ${item.label}`,
        kind: item.kind,
        subkind: item.name || null,
        hasChildren: false,
        annotations: [],
        properties: [],
        inlineChildren: [],
        refs: [],
      },
    };

    nodes = [...nodes, newNode];
  }

  function handleDragOver(event: DragEvent) {
    event.preventDefault();
    if (event.dataTransfer) {
      event.dataTransfer.dropEffect = 'move';
    }
  }

  /**
   * Handle "Create Implementation" button click. Calls the backend to create
   * an impl-shaped construct in the .veil source, then navigates to the target
   * group tab. Entirely layer-driven — no domain knowledge.
   */
  async function handleImplement(implEntry: any, targetNodeName: string) {
    // Find the parent context node's span (we need it to create a child construct)
    const graph = $irGraph;
    const parent = $currentParent;
    console.log('[handleImplement] start', { parent, targetNodeName, implEntry: implEntry.name, dg: implEntry.dg });
    if (!graph || !parent) { console.log('[handleImplement] no graph or parent'); return; }
    const parentNode = graph.nodes.find(n => n.id === parent);
    if (!parentNode) { console.log('[handleImplement] no parentNode for id', parent); return; }
    console.log('[handleImplement] parentNode', parentNode.name, 'span', parentNode.span.start);

    const implName = `${targetNodeName}${implEntry.label}`;
    const targetGroup = implEntry.dg;

    // Find the target group node to insert into (dg = default group)
    let insertParentSpan = parentNode.span.start;
    if (targetGroup) {
      const groupNode = graph.nodes.find(
        (n: any) => n.kind === 'Group' && n.name === targetGroup && n.metadata.parent === parent
      );
      if (groupNode) {
        insertParentSpan = groupNode.span.start;
      } else {
        // Group doesn't exist yet — create it first, then insert into it
        const createGroupSuccess = await saveEdits([{
          op: 'create_construct',
          parent_span: parentNode.span.start,
          keyword: 'group',
          name: targetGroup,
        }]);
        console.log('[handleImplement] group creation result:', createGroupSuccess);
        if (!createGroupSuccess) return;
        // After creating the group, find its span in the fresh IR
        const freshGraph = $irGraph;
        if (!freshGraph) return;
        const newGroupNode = freshGraph.nodes.find(
          (n: any) => n.kind === 'Group' && n.name === targetGroup && n.metadata.parent === parent
        );
        if (newGroupNode) {
          insertParentSpan = newGroupNode.span.start;
        }
      }
    }

    // Call backend to create the impl construct in the source
    // Set the active tab AFTER saving completes so the $effect uses it
    // on the next render cycle (avoids effect_update_depth_exceeded).
    const success = await saveEdits([{
      op: 'create_construct',
      parent_span: insertParentSpan,
      keyword: implEntry.keyword,
      name: implName,
      target: targetNodeName,
    }]);

    if (success && targetGroup) {
      // Defer tab switch to next tick to avoid reactive loop
      setTimeout(() => {
        activeTab = targetGroup;
        switchTab(targetGroup);
      }, 0);
    } else if (!success && targetGroup) {
      console.log('[handleImplement] adapter creation FAILED');
    } else {
      console.log('[handleImplement] adapter creation SUCCESS');
    }
  }

  onMount(() => {
    fetchIr();
  });

  // Recompute nodes/edges when graph or current parent changes
  $effect(() => {
    const graph = $irGraph;
    const parent = $currentParent;
    if (!graph) return;
    untrack(() => computeView(graph, parent, []));
  });

  function switchTab(tab: string) {
    activeTab = tab;
    const graph = $irGraph;
    const parent = $currentParent;
    if (graph) computeView(graph, parent);
  }

  function computeView(graph: IrGraph, parentId: number | null, prevNodes: Node[] = []) {
    let children = getChildren(graph, parentId);

    // Filter out layer-provided infrastructure unless toggled on
    if (!showLayerProvided) {
      children = children.filter(c => !c.metadata.annotations.includes('layer-provided'));
    }

    const visibleIds = new Set(children.map(c => c.id));

    // Check if we're at the Solution level with modules + cross-module flows
    const parentNode = parentId ? graph.nodes.find(n => n.id === parentId) : null;
    const isSolutionLevel = !parentNode || parentNode.kind === 'Solution';
    const modules = children.filter(c => c.kind === 'Module');
    // Any node with a reference line (`ref:*`, e.g. a saga's `contexts`) spans
    // modules — layer-agnostic, no hardcoded keyword.
    const spanning = children.filter(c => c.metadata.properties.some(([k]) => k.startsWith('ref:')));

    // Simple flat view — modules and flows as regular nodes
    if (isSolutionLevel && modules.length > 0) {
    // Spanning nodes get edges to the modules they reference
    const solNodes: Node[] = children.map(child => {
      const childChildren = getChildren(graph, child.id);
      const refs = getCrossRefs(graph, child.id, visibleIds);

      return {
        id: String(child.id),
        type: 'veil',
        position: { x: 0, y: 0 },
        data: {
          label: child.name,
          kind: child.kind,
          subkind: child.metadata.subkind,
          hasChildren: childChildren.length > 0,
          annotations: child.metadata.annotations,
          properties: child.metadata.properties,
          refs,
        },
      };
    });

    // Edges: spanning node → modules it references
    const solEdges: Edge[] = [];

    for (const span of spanning) {
      const ctxRefs = span.metadata.properties.find(([k]) => k.startsWith('ref:'));
      if (ctxRefs) {
        const ctxNames = ctxRefs[1].split(', ');
        for (const ctxName of ctxNames) {
          const ctxNode = modules.find(c => c.name === ctxName);
          if (ctxNode) {
            solEdges.push({
              id: `span-${span.id}-${ctxNode.id}`,
              source: String(span.id),
              target: String(ctxNode.id),
              animated: true,
              style: 'stroke: #dc2626; stroke-width: 2.5; stroke-dasharray: 6 3;',
              label: 'spans',
              labelStyle: 'font-size: 9px; fill: #dc2626;',
            });
          }
        }
      }
    }

    nodes = layoutByType(solNodes);
    edges = solEdges;
    tabs = [];
    activeTab = null;
    return;
    }

    // Standard flat view for other levels
    // Check if children contain groups — if so, use tabs.
    // Also check for expected groups declared in the layer (via requires_groups)
    // so we show tabs even for groups that don't have children yet.
    const groupNodes = children.filter(c => c.kind === 'Group');

    // Get expected groups from the layer config for this parent's subkind
    const parentSubkind = parentNode?.metadata.subkind ?? null;
    const paletteEntry = parentSubkind
      ? $paletteConfig.find((p: any) => p.name === parentSubkind)
      : null;
    const expectedGroups: string[] = paletteEntry?.expected_groups ?? [];

    // Merge: actual group nodes + expected groups that don't exist yet
    const allGroupNames = [...new Set([
      ...groupNodes.map(g => g.name),
      ...expectedGroups,
    ])];

    if (allGroupNames.length > 0) {
      // Sort to match the expected order from the layer
      if (expectedGroups.length > 0) {
        allGroupNames.sort((a, b) => {
          const ai = expectedGroups.indexOf(a);
          const bi = expectedGroups.indexOf(b);
          return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
        });
      }
      tabs = allGroupNames;
      // Use activeTab if valid, otherwise default to first
      let currentTab = activeTab;
      if (!currentTab || !tabs.includes(currentTab)) {
        currentTab = tabs[0];
        activeTab = currentTab;
      }

      const activeGroup = groupNodes.find(g => g.name === currentTab);
      if (activeGroup) {
        const groupChildren = getChildren(graph, activeGroup.id);
        // Also include non-group items at this level
        const nonGroupItems = children.filter(c => c.kind !== 'Group');
        const allItems = [...groupChildren, ...nonGroupItems];
        const itemIds = new Set(allItems.map(c => c.id));

        const tabNodes: Node[] = allItems.map(child => {
          const childChildren = getChildren(graph, child.id);
          const refs = getCrossRefs(graph, child.id, itemIds);
          return {
            id: String(child.id),
            type: 'veil',
            position: { x: 0, y: 0 },
            data: {
              label: child.name,
              kind: child.kind,
              subkind: child.metadata.subkind,
              hasChildren: childChildren.length > 0,
              annotations: child.metadata.annotations,
              properties: child.metadata.properties,
              refs,
            },
          };
        });

        const tabEdges: Edge[] = graph.edges
          .filter(e => itemIds.has(e.from) && itemIds.has(e.to))
          .filter(e => e.kind !== 'Contains')
          .map((e, i) => ({
            id: `e-${e.from}-${e.to}-${i}`,
            source: String(e.from),
            target: String(e.to),
            animated: e.kind === 'SequenceFlow',
            style: getEdgeStyle(e.kind),
          }));

        nodes = layoutByType(tabNodes);
        edges = tabEdges;
        return;
      }
      // Active tab is an expected group with no content yet — show empty canvas
      nodes = layoutByType([]);
      edges = [];
      return;
    } else {
      tabs = [];
      activeTab = null;
    }

    const flowNodes: Node[] = children.map(child => {
      const childChildren = getChildren(graph, child.id);
      const refs = getCrossRefs(graph, child.id, visibleIds);

      let inlineChildren: { name: string; kind: string; properties: [string, string][] }[] = [];
      let hasChildren = childChildren.length > 0;
      if (child.kind === 'ParallelGateway') {
        inlineChildren = childChildren.map(c => ({
          name: c.name,
          kind: c.kind,
          properties: c.metadata.properties,
        }));
        hasChildren = false;
      }

      return {
        id: String(child.id),
        type: 'veil',
        position: { x: 0, y: 0 },
        data: {
          label: child.name,
          kind: child.kind,
          subkind: child.metadata.subkind,
          spanStart: child.span.start,
          layerProvided: child.metadata.annotations.includes('layer-provided'),
          hasChildren,
          annotations: child.metadata.annotations,
          properties: child.metadata.properties,
          inlineChildren,
          refs,
        },
      };
    });

    // Edges between visible nodes
    const flowEdges: Edge[] = graph.edges
      .filter(e => visibleIds.has(e.from) && visibleIds.has(e.to))
      .filter(e => e.kind !== 'Contains')
      .map((e, i) => ({
        id: `e-${e.from}-${e.to}-${i}`,
        source: String(e.from),
        target: String(e.to),
        animated: e.kind === 'SequenceFlow',
        style: getEdgeStyle(e.kind),
        label: e.kind === 'Implements' ? 'implements' : e.kind === 'SequenceFlow' ? '' : e.kind,
        labelStyle: 'font-size: 10px; fill: #64748b;',
      }));

    // Ghost nodes for cross-references
    const ghostNodes: Node[] = [];
    const ghostEdges: Edge[] = [];
    let ghostIdx = 0;
    for (const child of children) {
      const outEdges = graph.edges.filter(
        e => e.from === child.id && !visibleIds.has(e.to) && e.kind !== 'Contains'
      );
      for (const e of outEdges) {
        const targetNode = graph.nodes.find(n => n.id === e.to);
        if (!targetNode) continue;
        const ghostId = `ghost-${ghostIdx++}`;
        ghostNodes.push({
          id: ghostId, type: 'veil', position: { x: 0, y: 0 },
          data: { label: targetNode.name, kind: targetNode.kind, hasChildren: false, annotations: [], isGhost: true },
        });
        ghostEdges.push({
          id: `ge-${child.id}-${ghostId}`, source: String(child.id), target: ghostId,
          animated: false, style: getEdgeStyle(e.kind),
        });
      }
    }

    const allNodes = [...flowNodes, ...ghostNodes];
    const allEdges = [...flowEdges, ...ghostEdges];

    const direction = parentNode?.kind === 'Flow' || parentNode?.kind === 'ParallelGateway'
      ? 'LR' : 'TB';

    const isFlowView = parentNode?.kind === 'Flow'
      || parentNode?.kind === 'ParallelGateway' || parentNode?.kind === 'Step';

    if (isFlowView) {
      nodes = layoutNodes(allNodes, allEdges, direction);
    } else {
      nodes = layoutByType(allNodes);
    }
    edges = allEdges;
  }

  /** Layout nodes in vertical columns grouped by subkind/kind. */
  function layoutByType(flowNodes: Node[]): Node[] {
    return doColumnLayout(flowNodes);
  }

  /** Pure column layout (used for initial load when no positions exist) */
  function doColumnLayout(flowNodes: Node[]): Node[] {
    const NODE_W = 240;
    const NODE_H = 140;    // account for badges, details button
    const V_GAP = 30;      // vertical gap between same-type nodes
    const COL_GAP = 80;    // horizontal gap between type columns
    const MAX_PER_COL = 6; // wrap to new column after this many

    // Group nodes by their display type (subkind or kind)
    const groups: Record<string, Node[]> = {};
    for (const node of flowNodes) {
      const type = String(node.data.subkind ?? node.data.kind ?? 'Other');
      if (!groups[type]) groups[type] = [];
      groups[type].push(node);
    }

    // Layout each group as a vertical column
    let colX = 0;
    const result: Node[] = [];

    for (const [_, groupNodes] of Object.entries(groups)) {
      let x = colX;
      let y = 0;
      let colCount = 0;

      for (const node of groupNodes) {
        if (colCount >= MAX_PER_COL) {
          // Wrap to a new sub-column
          x += NODE_W + V_GAP;
          y = 0;
          colCount = 0;
        }

        result.push({
          ...node,
          position: { x, y },
        });

        y += NODE_H + V_GAP;
        colCount++;
      }

      // Move to next type column
      const colsUsed = Math.ceil(groupNodes.length / MAX_PER_COL);
      colX += colsUsed * (NODE_W + V_GAP) + COL_GAP;
    }

    return result;
  }

  function getEdgeStyle(kind: string): string {
    switch (kind) {
      case 'Implements':
        return 'stroke: #a855f7; stroke-width: 2; stroke-dasharray: 6 3;';
      case 'SequenceFlow':
        return 'stroke: #6366f1; stroke-width: 2;';
      case 'Calls':
        return 'stroke: #10b981; stroke-width: 1.5; stroke-dasharray: 4 2;';
      case 'Emits':
        return 'stroke: #f59e0b; stroke-width: 1.5; stroke-dasharray: 3 3;';
      default:
        return 'stroke: #475569; stroke-width: 1.5;';
    }
  }

  function getCrossRefs(graph: IrGraph, nodeId: number, visibleIds: Set<number>): string[] {
    const refs: string[] = [];
    const outEdges = graph.edges.filter(
      e => e.from === nodeId && !visibleIds.has(e.to) && e.kind !== 'Contains'
    );
    for (const e of outEdges) {
      const target = graph.nodes.find(n => n.id === e.to);
      if (target) {
        refs.push(`${e.kind.toLowerCase()}: ${target.name}`);
      }
    }
    return refs;
  }

  function handleNodeClick({ node, event }: { node: Node; event: MouseEvent | TouchEvent }) {
    const graph = $irGraph;
    if (!graph) return;
    const irNode = graph.nodes.find(n => n.id === Number(node.id));

    // Always update selection
    selectedNodeId.set(node.id);

    // Double-click to drill down
    if (irNode && event instanceof MouseEvent && event.detail === 2) {
      const children = getChildren(graph, irNode.id);
      if (children.length > 0) {
        drillDown(irNode);
        selectedNodeId.set(null);
      }
    }
  }

  function handleKeyDown(event: KeyboardEvent) {
    // Don't act if user is typing in an input/textarea
    const tag = (event.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;

    // Enter to drill into selected node (same as double-click)
    if (event.key === 'Enter' && $selectedNodeId) {
      const graph = $irGraph;
      if (!graph) return;
      const irNode = graph.nodes.find(n => n.id === Number($selectedNodeId));
      if (irNode) {
        const children = getChildren(graph, irNode.id);
        if (children.length > 0) {
          drillDown(irNode);
          selectedNodeId.set(null);
        }
      }
    }

    if ((event.key === 'Delete' || event.key === 'Backspace') && $selectedNodeId) {
      nodes = nodes.filter(n => n.id !== $selectedNodeId);
      edges = edges.filter(e => e.source !== $selectedNodeId && e.target !== $selectedNodeId);
      selectedNodeId.set(null);
    }
  }

  function handleConnect(connection: { source: string; target: string }) {
    const newEdge: Edge = {
      id: `e-${connection.source}-${connection.target}-${Date.now()}`,
      source: connection.source,
      target: connection.target,
      animated: true,
      style: 'stroke: #6366f1; stroke-width: 2;',
    };
    edges = [...edges, newEdge];
  }

  function handlePaneClick() {
    // Only deselect, don't interfere with anything else
    if ($selectedNodeId) {
      selectedNodeId.set(null);
    }
  }
</script>

<svelte:window onkeydown={handleKeyDown} />

<div class="viewer-container">
  <!-- Top bar -->
  <div class="top-bar">
    <div class="breadcrumbs">
      {#each $breadcrumbs as crumb, i}
        {#if i > 0}
          <span class="breadcrumb-sep">›</span>
        {/if}
        <button
          class="breadcrumb-item"
          class:active={i === $breadcrumbs.length - 1}
          onclick={() => navigateTo(crumb.id)}
        >
          {crumb.name}
        </button>
      {/each}
    </div>
    <label class="layer-toggle">
      <input type="checkbox" bind:checked={showLayerProvided} onchange={() => { const g = $irGraph; const p = $currentParent; if (g) computeView(g, p); }} />
      <span>Show infrastructure</span>
    </label>
  </div>

  {#if $loading}
    <div class="status-overlay">
      <div class="pulse-ring"></div>
      <p>Loading...</p>
    </div>
  {:else if $error}
    <div class="status-overlay error">
      <p class="error-title">⚠️ Connection Error</p>
      <p class="error-msg">{$error}</p>
      <p class="error-hint">
        Run: <code>cargo run -- serve examples/customer_onboarding.veil</code>
      </p>
      <button class="retry-btn" onclick={() => fetchIr()}>Retry</button>
    </div>
  <!-- Scope panel — shows variables available at current level -->
  {:else}
    {#if scopeVars.length > 0}
      <div class="scope-bar">
        <span class="scope-label">Scope:</span>
        {#each scopeVars as v}
          <span class="scope-var">{v}</span>
        {/each}
      </div>
    {/if}
    <div class="main-layout">
      <Palette contextKind={currentContextKind} activeGroup={activeTab} />
      <div class="graph-wrapper">
        {#if tabs.length > 0}
          <div class="tab-bar">
            {#each tabs as tab}
              <button
                class="tab-btn"
                class:active={activeTab === tab}
                onclick={() => switchTab(tab)}
              >
                {tab}
              </button>
            {/each}
          </div>
        {/if}
        <div class="graph-container" ondrop={handleDrop} ondragover={handleDragOver} role="application" onkeydown={handleKeyDown} tabindex="-1">
        <SvelteFlow
          {nodes}
          {edges}
          {nodeTypes}
          fitView
          onnodeclick={handleNodeClick}
          onconnect={handleConnect}
          onpaneclick={handlePaneClick}
          colorMode="dark"
        >
          <Background variant={BackgroundVariant.Dots} gap={20} size={1} />
          <Controls />
          <MiniMap />
        </SvelteFlow>

        {#if selectedNode}
          <PropertyEditor
            node={selectedNode}
            onUpdate={updateNodeData}
            onClose={() => selectedNodeId.set(null)}
            onImplement={handleImplement}
          />
        {/if}
      </div>
      </div>
    </div>
  {/if}
  <CodePreview />
</div>

<style>
  .viewer-container {
    width: 100vw;
    height: 100vh;
    display: flex;
    flex-direction: column;
    background: #0a0a0f;
  }

  .top-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 20px;
    background: rgba(26, 26, 46, 0.95);
    border-bottom: 1px solid #2d2d44;
    backdrop-filter: blur(12px);
    z-index: 10;
  }

  .layer-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: #64748b;
    cursor: pointer;
  }
  .layer-toggle input { accent-color: #6366f1; }
  .layer-toggle:hover { color: #94a3b8; }

  .breadcrumbs {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow-x: auto;
  }

  .breadcrumb-item {
    background: none;
    border: none;
    color: #94a3b8;
    font-size: 13px;
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 6px;
    transition: all 0.15s;
  }

  .breadcrumb-item:hover {
    background: rgba(99, 102, 241, 0.1);
    color: #e2e8f0;
  }

  .breadcrumb-item.active {
    color: #e2e8f0;
    font-weight: 600;
    background: rgba(99, 102, 241, 0.15);
  }

  .breadcrumb-sep {
    color: #475569;
    font-size: 14px;
  }

  .graph-container {
    flex: 1;
    min-height: 0;
    min-width: 0;
    position: relative;
  }

  .graph-wrapper {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
  }

  .tab-bar {
    display: flex;
    gap: 2px;
    padding: 8px 12px;
    background: rgba(26, 26, 46, 0.9);
    border-bottom: 1px solid #2d2d44;
  }

  .scope-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 16px;
    background: rgba(34, 197, 94, 0.05);
    border-bottom: 1px solid rgba(34, 197, 94, 0.2);
    overflow-x: auto;
    flex-shrink: 0;
  }

  .scope-label {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: #22c55e;
    font-weight: 700;
  }

  .scope-var {
    font-size: 11px;
    font-family: 'JetBrains Mono', monospace;
    padding: 2px 8px;
    border-radius: 4px;
    background: rgba(34, 197, 94, 0.08);
    border: 1px solid rgba(34, 197, 94, 0.2);
    color: #86efac;
  }

  .tab-btn {
    padding: 6px 14px;
    font-size: 11px;
    font-weight: 600;
    text-transform: capitalize;
    border: 1px solid #2d2d44;
    border-radius: 6px;
    background: transparent;
    color: #64748b;
    cursor: pointer;
    transition: all 0.15s;
  }

  .tab-btn:hover {
    background: rgba(99, 102, 241, 0.08);
    color: #94a3b8;
  }

  .tab-btn.active {
    background: rgba(99, 102, 241, 0.15);
    color: #a5b4fc;
    border-color: rgba(99, 102, 241, 0.4);
  }

  .main-layout {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .status-overlay {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: #94a3b8;
  }

  .status-overlay.error { color: #f87171; }
  .error-title { font-size: 18px; font-weight: 600; }
  .error-msg { font-size: 14px; color: #94a3b8; }
  .error-hint { font-size: 12px; color: #64748b; margin-top: 8px; }
  .error-hint code { color: #a5b4fc; background: rgba(99, 102, 241, 0.1); padding: 2px 6px; border-radius: 4px; }
  .retry-btn { margin-top: 12px; padding: 8px 20px; background: #6366f1; color: white; border: none; border-radius: 8px; cursor: pointer; }
  .retry-btn:hover { background: #4f46e5; }
  .pulse-ring { width: 40px; height: 40px; border-radius: 50%; border: 3px solid #6366f1; animation: pulse 1.5s infinite; }
  @keyframes pulse { 0% { transform: scale(1); opacity: 1; } 50% { transform: scale(1.3); opacity: 0.5; } 100% { transform: scale(1); opacity: 1; } }

  :global(.svelte-flow) { background: #0a0a0f !important; }
  :global(.svelte-flow__background) { opacity: 0.4; }
  :global(.svelte-flow__minimap) { background: rgba(26, 26, 46, 0.9) !important; border: 1px solid #2d2d44 !important; border-radius: 10px !important; }
  :global(.svelte-flow__controls) { background: rgba(26, 26, 46, 0.9) !important; border: 1px solid #2d2d44 !important; border-radius: 10px !important; }
  :global(.svelte-flow__controls button) { background: transparent !important; border-color: #2d2d44 !important; color: #e2e8f0 !important; }
  :global(.svelte-flow__controls button:hover) { background: rgba(99, 102, 241, 0.15) !important; }
  :global(.svelte-flow__edge-path) { stroke-width: 2px; filter: drop-shadow(0 0 3px rgba(99, 102, 241, 0.3)); }
  :global(.svelte-flow__edge.animated .svelte-flow__edge-path) { stroke: #6366f1 !important; stroke-width: 2.5px; filter: drop-shadow(0 0 6px rgba(99, 102, 241, 0.5)); }
  :global(.svelte-flow__handle) { width: 8px !important; height: 8px !important; background: var(--node-color, #475569) !important; border: 2px solid #1a1a2e !important; opacity: 0; transition: opacity 0.2s; }
  :global(.svelte-flow__node:hover .svelte-flow__handle) { opacity: 1; }
</style>
