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
  import { layoutNodes, layoutByType } from '$lib/layout';
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
    presentationModel,
    saveEdits,
    availableFiles,
    activeFileName,
    selectFile,
  } from '$lib/store';
  import { NODE_STYLES, type IrNode, type IrGraph, type NodeKind } from '$lib/types';
  import {
    projectView,
    pickDefaultView,
    viewsForHost,
    irChildren,
    type ViewSpec,
  } from '$lib/presentation';

  const nodeTypes: NodeTypes = {
    veil: VeilNode as any,
  };

  let nodes = $state.raw<Node[]>([]);
  let edges = $state.raw<Edge[]>([]);
  let nextNodeId = $state(1000);
  let flowKey = $state(0);
  let tabs = $state<string[]>([]);
  let activeTab = $state<string | null>(null);
  /** Layer presentation views for current host (LAY-003). */
  let hostViews = $state<ViewSpec[]>([]);
  let activeViewId = $state<string | null>(null);
  let showLayerProvided = $state(false);
  // DOM reference for node measurement — ELK needs real rendered sizes
  let graphContainerEl: HTMLElement | null = $state(null);
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

  function switchView(viewId: string) {
    activeViewId = viewId;
    activeTab = null; // reset group tab when switching presentation view
    const graph = get(irGraph);
    const parent = get(currentParent);
    const palette = get(paletteConfig);
    if (graph) computeView(graph, parent, palette);
  }

  /** Map IR nodes to SvelteFlow nodes (shared by presentation + legacy paths). */
  function toFlowNodes(
    graph: IrGraph,
    items: IrNode[],
    visibleIds: Set<number>
  ): Node[] {
    return items.map((child) => {
      const childChildren = getChildren(graph, child.id);
      const refs = getCrossRefs(graph, child.id, visibleIds);
      let inlineChildren: { name: string; kind: string; properties: [string, string][] }[] = [];
      let hasChildren = childChildren.length > 0;
      if (child.kind === 'ParallelGateway') {
        inlineChildren = childChildren.map((c) => ({
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
  }

  function edgesAmong(graph: IrGraph, visibleIds: Set<number>): Edge[] {
    return graph.edges
      .filter((e) => visibleIds.has(e.from) && visibleIds.has(e.to))
      .filter((e) => e.kind !== 'Contains' && e.kind !== 'References')
      .map((e, i) => ({
        id: `e-${e.from}-${e.to}-${i}`,
        source: String(e.from),
        target: String(e.to),
        animated: e.kind === 'SequenceFlow',
        style: getEdgeStyle(e.kind),
        label:
          e.kind === 'Implements'
            ? 'implements'
            : e.kind === 'SequenceFlow'
              ? ''
              : e.kind,
        labelStyle: 'font-size: 10px; fill: var(--veil-text-dim);',
      }));
  }

  let computeInProgress = false;
  async function computeView(graph: IrGraph, parentId: number | null, palette: any[] = []) {
    let children = getChildren(graph, parentId);

    // Filter out layer-provided infrastructure unless toggled on
    if (!showLayerProvided) {
      children = children.filter((c) => !c.metadata.annotations.includes('layer-provided'));
    }

    const parentNode = parentId ? graph.nodes.find((n) => n.id === parentId) : null;
    currentContextKind = parentNode?.metadata.subkind ?? parentNode?.kind ?? 'Solution';
    currentContextKindCore = parentNode?.kind ?? 'Solution';
    const isSolutionLevel = !parentNode || parentNode.kind === 'Solution';
    const modules = children.filter((c) => c.kind === 'Module');
    const spanning = children.filter((c) =>
      c.metadata.properties.some(([k]) => k.startsWith('ref:'))
    );

    // Solution-level modules (unchanged special case)
    if (isSolutionLevel && modules.length > 0) {
      hostViews = [];
      activeViewId = null;
      const visibleIds = new Set(children.map((c) => c.id));
      const solNodes = toFlowNodes(graph, children, visibleIds);
      const solEdges: Edge[] = [];
      for (const span of spanning) {
        const ctxRefs = span.metadata.properties.find(([k]) => k.startsWith('ref:'));
        if (ctxRefs) {
          for (const ctxName of ctxRefs[1].split(', ')) {
            const ctxNode = modules.find((c) => c.name === ctxName.trim());
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
      nodes = await layoutByType(solNodes, graphContainerEl);
      edges = solEdges;
      tabs = [];
      activeTab = null;
      return;
    }

    // ─── LAY-003: layer presentation views ─────────────────────────────
    const pres = get(presentationModel);
    const hostName = parentNode?.metadata.subkind ?? null;
    const views = viewsForHost(pres, hostName);
    hostViews = views;

    if (views.length > 0 && parentId != null) {
      const hostDto = hostName && pres ? pres.hosts[hostName] : undefined;
      if (!activeViewId || !views.some((v) => v.id === activeViewId)) {
        activeViewId = pickDefaultView(hostDto, views);
      }
      const view = views.find((v) => v.id === activeViewId) ?? views[0];
      const projected = projectView(graph, parentId, view, {
        hideLayerProvided: !showLayerProvided,
      });

      if (projected.layout === 'tabs') {
        tabs = projected.tabs;
        let currentTab = activeTab;
        if (!currentTab || !tabs.includes(currentTab)) {
          currentTab = tabs[0] ?? null;
          activeTab = currentTab;
        }
        const groupNode = currentTab
          ? projected.tabGroupNodes.get(currentTab)
          : null;
        let allItems: IrNode[] = [];
        if (groupNode) {
          let gc = irChildren(graph, groupNode.id);
          if (!showLayerProvided) {
            gc = gc.filter((c) => !c.metadata.annotations.includes('layer-provided'));
          }
          const nonGroup = children.filter((c) => c.kind !== 'Group');
          allItems = [...gc, ...nonGroup];
        } else if (currentTab) {
          // Virtual empty tab (expected group not yet in source)
          allItems = [];
        }
        const itemIds = new Set(allItems.map((c) => c.id));
        const tabNodes = toFlowNodes(graph, allItems, itemIds);
        nodes = await layoutByType(tabNodes, graphContainerEl);
        edges = edgesAmong(graph, itemIds);
        return;
      }

      // flat | tree | flow — show projected top-level nodes
      tabs = [];
      activeTab = null;
      const itemIds = new Set(projected.nodes.map((c) => c.id));
      const flowNodes = toFlowNodes(graph, projected.nodes, itemIds);
      const flowEdges = edgesAmong(graph, itemIds);
      const useFlowLayout =
        projected.layout === 'flow' ||
        parentNode?.kind === 'Flow' ||
        parentNode?.kind === 'Step' ||
        parentNode?.kind === 'InterfaceMethod';
      if (useFlowLayout) {
        nodes = await layoutNodes(flowNodes, flowEdges, 'LR', graphContainerEl);
      } else {
        nodes = await layoutByType(flowNodes, graphContainerEl);
      }
      edges = flowEdges;
      return;
    }

    // ─── Fallback: no presentation (legacy requires_groups / flat) ─────
    hostViews = [];
    activeViewId = null;
    const visibleIds = new Set(children.map((c) => c.id));

    const groupNodes = children.filter((c) => c.kind === 'Group');
    const parentSubkind = parentNode?.metadata.subkind ?? null;
    const paletteEntry = parentSubkind
      ? palette.find((p: any) => p.name === parentSubkind)
      : null;
    const expectedGroups: string[] = paletteEntry?.expected_groups ?? [];
    const allGroupNames = [
      ...new Set([...groupNodes.map((g) => g.name), ...expectedGroups]),
    ];

    if (allGroupNames.length > 0) {
      if (expectedGroups.length > 0) {
        allGroupNames.sort((a, b) => {
          const ai = expectedGroups.indexOf(a);
          const bi = expectedGroups.indexOf(b);
          return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
        });
      }
      tabs = allGroupNames;
      let currentTab = activeTab;
      if (!currentTab || !tabs.includes(currentTab)) {
        currentTab = tabs[0];
        activeTab = currentTab;
      }
      const activeGroup = groupNodes.find((g) => g.name === currentTab);
      if (activeGroup) {
        const groupChildren = getChildren(graph, activeGroup.id);
        const nonGroupItems = children.filter((c) => c.kind !== 'Group');
        const allItems = [...groupChildren, ...nonGroupItems];
        const itemIds = new Set(allItems.map((c) => c.id));
        nodes = await layoutByType(toFlowNodes(graph, allItems, itemIds), graphContainerEl);
        edges = edgesAmong(graph, itemIds);
        return;
      }
      nodes = await layoutByType([], graphContainerEl);
      edges = [];
      return;
    }

    tabs = [];
    activeTab = null;

    const flowNodes = toFlowNodes(graph, children, visibleIds);
    const flowEdges = edgesAmong(graph, visibleIds);
    const ghostNodes: Node[] = [];
    const ghostEdges: Edge[] = [];
    let ghostIdx = 0;
    for (const child of children) {
      const outEdges = graph.edges.filter(
        (e) => e.from === child.id && !visibleIds.has(e.to) && e.kind !== 'Contains'
      );
      for (const e of outEdges) {
        const targetNode = graph.nodes.find((n) => n.id === e.to);
        if (!targetNode) continue;
        const ghostId = `ghost-${ghostIdx++}`;
        ghostNodes.push({
          id: ghostId,
          type: 'veil',
          position: { x: 0, y: 0 },
          data: {
            label: targetNode.name,
            kind: targetNode.kind,
            hasChildren: false,
            annotations: [],
            isGhost: true,
          },
        });
        ghostEdges.push({
          id: `ge-${child.id}-${ghostId}`,
          source: String(child.id),
          target: ghostId,
          animated: false,
          style: getEdgeStyle(e.kind),
        });
      }
    }

    const allNodes = [...flowNodes, ...ghostNodes];
    const allEdges = [...flowEdges, ...ghostEdges];
    const isFlowView =
      parentNode?.kind === 'Flow' ||
      parentNode?.kind === 'ParallelGateway' ||
      parentNode?.kind === 'Step' ||
      parentNode?.kind === 'InterfaceMethod';

    if (isFlowView) {
      nodes = await layoutNodes(allNodes, allEdges, 'LR', graphContainerEl);
    } else {
      nodes = await layoutByType(allNodes, graphContainerEl);
    }
    edges = allEdges;
  }

  /** Layout nodes in vertical columns grouped by subkind/kind. */
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
      event.preventDefault();
      void handleDeleteSelected();
    }
  }

  /** SER-006: persist delete via EditOp (not a local-only canvas filter). */
  async function handleDeleteSelected() {
    const id = get(selectedNodeId);
    if (!id) return;
    const graph = get(irGraph);
    if (!graph) return;

    // Ghost nodes are not real AST targets
    const flowNode = nodes.find(n => n.id === id);
    if (flowNode?.data?.isGhost) return;

    const irNode = graph.nodes.find(n => n.id === Number(id));
    if (!irNode) {
      // Unsaved dropped node — local remove only
      nodes = nodes.filter(n => n.id !== id);
      edges = edges.filter(e => e.source !== id && e.target !== id);
      selectedNodeId.set(null);
      return;
    }

    const layerProvided =
      irNode.metadata.annotations.includes('layer-provided')
      || Boolean(flowNode?.data?.layerProvided);
    if (layerProvided) {
      alert(`Cannot delete "${irNode.name}": layer-provided infrastructure.`);
      return;
    }

    const spanStart = irNode.span?.start ?? flowNode?.data?.spanStart;
    if (spanStart === undefined || spanStart === null) {
      alert(`Cannot delete "${irNode.name}": missing AST span (not yet saved?).`);
      return;
    }

    const kind = irNode.metadata.subkind || irNode.kind;
    if (!confirm(`Delete ${kind} "${irNode.name}"?\n\nThis will update the .veil source.`)) {
      return;
    }

    selectedNodeId.set(null);
    const ok = await saveEdits([{ op: 'delete_construct', span_start: spanStart }]);
    if (!ok) {
      // saveError is set by the store; keep selection cleared
      return;
    }
    const fresh = get(irGraph);
    const parent = get(currentParent);
    const palette = get(paletteConfig);
    if (fresh) {
      await computeView(fresh, parent, palette);
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
        {#if hostViews.length > 1}
          <div class="view-bar" role="tablist" aria-label="Presentation views">
            {#each hostViews as v}
              <button
                type="button"
                class="view-btn"
                class:active={activeViewId === v.id}
                role="tab"
                aria-selected={activeViewId === v.id}
                onclick={() => switchView(v.id)}
              >
                {v.label || v.id}
              </button>
            {/each}
          </div>
        {/if}
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
        <div class="graph-container" bind:this={graphContainerEl} ondrop={handleDrop} ondragover={handleDragOver} role="application" onkeydown={handleKeyDown} tabindex="-1">
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

  .view-bar {
    display: flex;
    gap: 4px;
    padding: 8px 12px 4px;
    background: var(--veil-surface);
    border-bottom: 1px solid var(--veil-border);
  }

  .view-btn {
    padding: 5px 12px;
    font-size: 11px;
    font-weight: 600;
    border: 1px solid transparent;
    border-radius: 999px;
    background: transparent;
    color: var(--veil-text-dim);
    cursor: pointer;
    transition: all 0.15s;
  }

  .view-btn:hover {
    background: var(--veil-accent-subtle);
    color: var(--veil-text-secondary);
  }

  .view-btn.active {
    background: var(--veil-accent-hover);
    color: var(--veil-text);
    border-color: rgba(115, 115, 115, 0.35);
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
