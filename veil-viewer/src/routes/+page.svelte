<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
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
  import DiagnosticsPanel from '$lib/DiagnosticsPanel.svelte';
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
    availableFiles,
    activeFileName,
    selectFile,
  } from '$lib/store';
  import { NODE_STYLES, type IrNode, type IrGraph, type NodeKind } from '$lib/types';

  const nodeTypes: NodeTypes = {
    veil: VeilNode as any,
  };

  let nodes = $state.raw<Node[]>([]);
  let edges = $state.raw<Edge[]>([]);
  let nextNodeId = $state(1000);
  let flowKey = $state(0);
  let tabs = $state<string[]>([]);
  let activeTab = $state<string | null>(null);
  let showLayerProvided = $state(false);
  let theme = $state<'dark' | 'light'>(
    (typeof localStorage !== 'undefined' && localStorage.getItem('veil-theme') as 'dark' | 'light') || 'dark'
  );

  function toggleTheme() {
    theme = theme === 'dark' ? 'light' : 'dark';
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem('veil-theme', theme);
  }

  // Derive the current context kind for palette filtering
  let currentContextKind = $state<string>('Solution');
  let currentContextKindCore = $state<string>('Solution');

  // Scope variables computed inside computeView (was $derived, moved to avoid reactive loops)
  let scopeVars = $state<string[]>([]);

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
    // Close the property editor FIRST — before any saveEdits calls.
    // PropertyEditor has a $effect reading $irGraph that causes loops if still mounted.
    selectedNodeId.set(null);
    // Hide SvelteFlow during save to prevent xyflow effects from looping
    // Two frames to ensure Svelte unmounts PropertyEditor and SvelteFlow
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));

    // Find the parent context node's span (we need it to create a child construct)
    const graph = get(irGraph);
    const parent = get(currentParent);
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
        // Remount xyflow after structural change
        flowKey += 1;
        // Refresh the view after group creation
        await new Promise(r => setTimeout(r, 0));
        const gAfterGroup = get(irGraph);
        if (gAfterGroup) computeView(gAfterGroup, get(currentParent), get(paletteConfig));
        // Fetch the fresh IR directly to find the new group's span,
        // avoiding reactive store reads that could trigger effect loops.
        const freshRes = await fetch('http://localhost:3001/api/ir');
        const freshIr = await freshRes.json();
        const newGroupNode = freshIr.nodes.find(
          (n: any) => n.kind === 'Group' && n.name === targetGroup && n.metadata.parent === parent
        );
        if (newGroupNode) {
          insertParentSpan = newGroupNode.span.start;
        }
      }
    }

    // Skip the irGraph subscription during save to prevent the loop

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
      // Compute the view with new IR first (sets nodes/edges),
      // then remount xyflow with the fresh state.
      const freshGraph = get(irGraph);
      if (freshGraph) computeView(freshGraph, get(currentParent), get(paletteConfig));
      flowKey += 1;
      // Defer tab switch to after remount
      setTimeout(() => { activeTab = targetGroup; switchTab(targetGroup); flowKey += 1; }, 100);
    } else {
      // Re-show even on failure
      const g = get(irGraph);
      if (g) computeView(g, get(currentParent), get(paletteConfig));
    }
  }

  onMount(() => {
    fetchIr().then(() => {
      const graph = get(irGraph);
      if (graph) computeView(graph, get(currentParent), get(paletteConfig));
    });

    // Apply saved theme on mount
    document.documentElement.setAttribute('data-theme', theme);

    // No irGraph subscription — computeView is called explicitly by
    // fetchIr (on load) and handleImplement (on save).
    // This prevents any auto-triggered computeView → nodes assignment
    // that could loop with xyflow's bind:nodes write-back.
    const unsubParent = currentParent.subscribe((parent) => {
      const graph = get(irGraph);
      if (!graph) return;
      const palette = get(paletteConfig);
      computeView(graph, parent, palette);
    });

    return () => {
      unsubParent();
    };
  });

  function switchTab(tab: string) {
    activeTab = tab;
    const graph = get(irGraph);
    const parent = get(currentParent);
    const palette = get(paletteConfig);
    if (graph) computeView(graph, parent, palette);
  }

  let computeInProgress = false;
  function computeView(graph: IrGraph, parentId: number | null, palette: any[] = []) {
    let children = getChildren(graph, parentId);

    // Filter out layer-provided infrastructure unless toggled on
    if (!showLayerProvided) {
      children = children.filter(c => !c.metadata.annotations.includes('layer-provided'));
    }

    const visibleIds = new Set(children.map(c => c.id));

    // Check if we're at the Solution level with modules + cross-module flows
    const parentNode = parentId ? graph.nodes.find(n => n.id === parentId) : null;
    // Update context kind for palette filtering (was a $derived, moved here to avoid reactive loops)
    currentContextKind = parentNode?.metadata.subkind ?? parentNode?.kind ?? 'Solution';
    currentContextKindCore = parentNode?.kind ?? 'Solution';
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
      ? palette.find((p: any) => p.name === parentSubkind)
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
          .filter(e => e.kind !== 'Contains' && e.kind !== 'References')
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
        labelStyle: 'font-size: 10px; fill: var(--veil-text-dim);',
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
        return 'stroke: var(--veil-text-secondary); stroke-width: 2; stroke-dasharray: 6 3;';
      case 'References':
        return 'stroke: #60a5fa; stroke-width: 1.5; stroke-dasharray: 4 2;';
      case 'SequenceFlow':
        return 'stroke: var(--veil-text-dim); stroke-width: 2;';
      case 'Calls':
        return 'stroke: var(--veil-text-dim); stroke-width: 1.5; stroke-dasharray: 4 2;';
      case 'Emits':
        return 'stroke: var(--veil-text-dim); stroke-width: 1.5; stroke-dasharray: 3 3;';
      default:
        return 'stroke: var(--veil-text-faint); stroke-width: 1.5;';
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
    const graph = get(irGraph);
    if (!graph) return;
    const irNode = graph.nodes.find(n => n.id === Number(node.id));

    // Always update selection
    selectedNodeId.set(node.id);

    // Show reference edges for the selected node, hide others
    updateReferenceEdges(graph, node.id);

    // Double-click to drill down
    if (irNode && event instanceof MouseEvent && event.detail === 2) {
      const children = getChildren(graph, irNode.id);
      if (children.length > 0) {
        drillDown(irNode);
        selectedNodeId.set(null);
      }
    }
  }

  /** Add/remove reference edges based on which node is selected */
  function updateReferenceEdges(graph: IrGraph, selectedId: string | null) {
    // Remove any existing reference edges
    edges = edges.filter(e => !e.id.startsWith('ref-'));

    if (!selectedId) return;

    // Build a position lookup from visible nodes
    const nodePositions = new Map<string, { x: number; y: number }>();
    for (const n of nodes) {
      nodePositions.set(n.id, n.position);
    }

    // Find reference edges that touch the selected node
    const nodeId = Number(selectedId);
    const visibleIds = new Set(nodes.map(n => Number(n.id)));
    const refEdges: Edge[] = graph.edges
      .filter(e => e.kind === 'References')
      .filter(e => (e.from === nodeId || e.to === nodeId))
      .filter(e => visibleIds.has(e.from) && visibleIds.has(e.to))
      .map((e, i) => {
        const sourcePos = nodePositions.get(String(e.from));
        const targetPos = nodePositions.get(String(e.to));
        // Determine shortest path handles based on relative position
        let sourceHandle = 'bottom';
        let targetHandle = 'top';
        if (sourcePos && targetPos) {
          const dx = targetPos.x - sourcePos.x;
          const dy = targetPos.y - sourcePos.y;
          if (Math.abs(dx) > Math.abs(dy)) {
            // Horizontal relationship dominates
            sourceHandle = dx > 0 ? 'right' : 'left';
            targetHandle = dx > 0 ? 'left' : 'right';
          } else {
            // Vertical relationship dominates
            sourceHandle = dy > 0 ? 'bottom' : 'top';
            targetHandle = dy > 0 ? 'top' : 'bottom';
          }
        }
        return {
          id: `ref-${e.from}-${e.to}-${i}`,
          source: String(e.from),
          target: String(e.to),
          sourceHandle,
          targetHandle,
          animated: false,
          style: getEdgeStyle('References'),
        };
      });

    if (refEdges.length > 0) {
      edges = [...edges, ...refEdges];
    }
  }

  function handleKeyDown(event: KeyboardEvent) {
    // Don't act if user is typing in an input/textarea
    const tag = (event.target as HTMLElement)?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return;

    // Enter to drill into selected node (same as double-click)
    if (event.key === 'Enter' && $selectedNodeId) {
      const graph = get(irGraph);
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
      style: 'stroke: var(--veil-text-dim); stroke-width: 2;',
    };
    edges = [...edges, newEdge];
  }

  function handlePaneClick() {
    // Only deselect, don't interfere with anything else
    if ($selectedNodeId) {
      selectedNodeId.set(null);
      // Remove reference edges on deselect
      updateReferenceEdges(get(irGraph)!, null);
    }
  }
</script>

<svelte:window onkeydown={handleKeyDown} />

<div class="viewer-container">
  <!-- Top bar -->
  <div class="top-bar">
    <div class="breadcrumbs">
      {#if $availableFiles.length > 1}
        <select
          class="file-selector"
          value={$availableFiles.findIndex(f => f.active)}
          onchange={(e) => selectFile(Number(e.target.value))}
        >
          {#each $availableFiles as file}
            <option value={file.index}>{file.name}</option>
          {/each}
        </select>
        <span class="breadcrumb-sep">›</span>
      {/if}
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
      <input type="checkbox" bind:checked={showLayerProvided} onchange={() => { const g = get(irGraph); const p = get(currentParent); if (g) computeView(g, p); }} />
      <span>Show infrastructure</span>
    </label>
    <button class="theme-toggle" onclick={toggleTheme} title="Toggle light/dark mode">
      {theme === 'dark' ? '☀️' : '🌙'}
    </button>
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
      <Palette contextKind={currentContextKind} contextKindCore={currentContextKindCore} activeGroup={activeTab} />
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
        <DiagnosticsPanel />
        
        {#key flowKey}
        <SvelteFlow
          bind:nodes
          bind:edges
          {nodeTypes}
          onnodeclick={handleNodeClick}
          onconnect={handleConnect}
          onpaneclick={handlePaneClick}
          colorMode={theme}
        >
          <Background variant={BackgroundVariant.Dots} gap={20} size={1} />
          <Controls />
          <MiniMap />
        </SvelteFlow>
        {/key}

        {#if selectedNode}
          <PropertyEditor
            node={selectedNode}
            onUpdate={updateNodeData}
            onClose={() => selectedNodeId.set(null)}
            
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
    background: var(--veil-bg);
  }

  .top-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 20px;
    background: var(--veil-surface-alt);
    border-bottom: 1px solid var(--veil-border);
    backdrop-filter: blur(12px);
    z-index: 10;
  }

  .layer-toggle {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--veil-text-dim);
    cursor: pointer;
  }
  .layer-toggle input { accent-color: var(--veil-text-dim); }
  .layer-toggle:hover { color: var(--veil-text-secondary); }

  .theme-toggle {
    background: none;
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 14px;
    cursor: pointer;
    transition: all 0.15s;
    line-height: 1;
  }
  .theme-toggle:hover {
    background: var(--veil-accent-hover);
  }

  .breadcrumbs {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow-x: auto;
  }

  .file-selector {
    background: var(--veil-input-bg);
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: 12px;
    font-weight: 600;
    color: var(--veil-text);
    cursor: pointer;
    outline: none;
  }
  .file-selector:focus { border-color: var(--veil-accent); }

  .breadcrumb-item {
    background: none;
    border: none;
    color: var(--veil-text-secondary);
    font-size: 13px;
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 6px;
    transition: all 0.15s;
  }

  .breadcrumb-item:hover {
    background: var(--veil-accent-subtle);
    color: var(--veil-text);
  }

  .breadcrumb-item.active {
    color: var(--veil-text);
    font-weight: 600;
    background: var(--veil-accent-hover);
  }

  .breadcrumb-sep {
    color: var(--veil-text-faint);
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
    background: var(--veil-surface-alt);
    border-bottom: 1px solid var(--veil-border);
  }

  .scope-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 16px;
    background: var(--veil-accent-subtle);
    border-bottom: 1px solid var(--veil-border);
    overflow-x: auto;
    flex-shrink: 0;
  }

  .scope-label {
    font-size: 9px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--veil-text-secondary);
    font-weight: 700;
  }

  .scope-var {
    font-size: 11px;
    font-family: 'JetBrains Mono', monospace;
    padding: 2px 8px;
    border-radius: 4px;
    background: var(--veil-accent-hover);
    border: 1px solid var(--veil-border);
    color: var(--veil-text);
  }

  .tab-btn {
    padding: 6px 14px;
    font-size: 11px;
    font-weight: 600;
    text-transform: capitalize;
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    background: transparent;
    color: var(--veil-text-dim);
    cursor: pointer;
    transition: all 0.15s;
  }

  .tab-btn:hover {
    background: var(--veil-accent-subtle);
    color: var(--veil-text-secondary);
  }

  .tab-btn.active {
    background: var(--veil-accent-hover);
    color: var(--veil-text);
    border-color: rgba(115, 115, 115, 0.4);
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
    color: var(--veil-text-secondary);
  }

  .status-overlay.error { color: #f87171; }
  .error-title { font-size: 18px; font-weight: 600; }
  .error-msg { font-size: 14px; color: var(--veil-text-secondary); }
  .error-hint { font-size: 12px; color: var(--veil-text-dim); margin-top: 8px; }
  .error-hint code { color: var(--veil-text); background: var(--veil-accent-subtle); padding: 2px 6px; border-radius: 4px; }
  .retry-btn { margin-top: 12px; padding: 8px 20px; background: var(--veil-text-faint); color: white; border: none; border-radius: 8px; cursor: pointer; }
  .retry-btn:hover { background: var(--veil-accent); }
  .pulse-ring { width: 40px; height: 40px; border-radius: 50%; border: 3px solid var(--veil-text-faint); animation: pulse 1.5s infinite; }
  @keyframes pulse { 0% { transform: scale(1); opacity: 1; } 50% { transform: scale(1.3); opacity: 0.5; } 100% { transform: scale(1); opacity: 1; } }

  :global(.svelte-flow) { background: var(--veil-bg) !important; }
  :global(.svelte-flow__background) { opacity: 0.4; }
  :global(.svelte-flow__minimap) { background: var(--veil-surface-alt) !important; border: 1px solid var(--veil-border) !important; border-radius: 10px !important; }
  :global(.svelte-flow__controls) { background: var(--veil-surface-alt) !important; border: 1px solid var(--veil-border) !important; border-radius: 10px !important; }
  :global(.svelte-flow__controls button) { background: transparent !important; border-color: var(--veil-border) !important; color: var(--veil-text) !important; }
  :global(.svelte-flow__controls button:hover) { background: var(--veil-accent-hover) !important; }
  :global(.svelte-flow__edge-path) { stroke-width: 2px; filter: drop-shadow(0 0 3px rgba(100, 100, 100, 0.2)); }
  :global(.svelte-flow__edge.animated .svelte-flow__edge-path) { stroke: var(--veil-text-dim) !important; stroke-width: 2.5px; filter: drop-shadow(0 0 6px rgba(100, 100, 100, 0.3)); }
  :global(.svelte-flow__handle) { width: 8px !important; height: 8px !important; background: var(--node-color, var(--veil-text-faint)) !important; border: 2px solid var(--veil-surface) !important; opacity: 0; transition: opacity 0.2s; }
  :global(.svelte-flow__node:hover .svelte-flow__handle) { opacity: 1; }
</style>
