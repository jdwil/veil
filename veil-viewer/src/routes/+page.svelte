<script lang="ts">
  import { onMount } from 'svelte';
  import {
    SvelteFlow,
    Controls,
    Background,
    BackgroundVariant,
    MiniMap,
    useSvelteFlow,
    type Node,
    type Edge,
    type NodeTypes,
  } from '@xyflow/svelte';
  import '@xyflow/svelte/dist/style.css';

  import VeilNode from '$lib/VeilNode.svelte';
  import Palette from '$lib/Palette.svelte';
  import PropertyEditor from '$lib/PropertyEditor.svelte';
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
  } from '$lib/store';
  import { NODE_STYLES, type IrNode, type IrGraph, type NodeKind } from '$lib/types';

  const nodeTypes: NodeTypes = {
    veil: VeilNode as any,
  };

  let nodes = $state<Node[]>([]);
  let edges = $state<Edge[]>([]);
  let nextNodeId = $state(1000);

  // Derive the current context kind for palette filtering
  let currentContextKind = $derived.by(() => {
    const graph = $irGraph;
    const parent = $currentParent;
    if (!graph || !parent) return 'Solution';
    const parentNode = graph.nodes.find(n => n.id === parent);
    return parentNode?.kind ?? 'Solution';
  });

  // Get the currently selected node for property editing
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

    const item = JSON.parse(data) as { kind: NodeKind; label: string; icon: string };

    // Create new node at drop position
    const id = String(nextNodeId++);
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    const position = {
      x: event.clientX - rect.left - 100,
      y: event.clientY - rect.top - 40,
    };

    const newNode: Node = {
      id,
      type: 'veil',
      position,
      data: {
        label: `New ${item.label}`,
        kind: item.kind,
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

  onMount(() => {
    fetchIr();
  });

  // Recompute nodes/edges when graph or current parent changes
  $effect(() => {
    const graph = $irGraph;
    const parent = $currentParent;
    if (!graph) return;
    computeView(graph, parent);
  });

  function computeView(graph: IrGraph, parentId: number | null) {
    const children = getChildren(graph, parentId);
    const visibleIds = new Set(children.map(c => c.id));

    // Check if we're at the Solution level with Contexts + Sagas
    const parentNode = parentId ? graph.nodes.find(n => n.id === parentId) : null;
    const isSolutionLevel = !parentNode || parentNode.kind === 'Solution';
    const contexts = children.filter(c => c.kind === 'Context');
    const sagas = children.filter(c => c.kind === 'Saga');

    // Use group node layout for solution-level when we have contexts
    if (isSolutionLevel && contexts.length > 0) {
      computeSolutionView(graph, children, contexts, sagas);
      return;
    }

    // Standard flat view for other levels
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
      || parentNode?.kind === 'Saga' ? 'LR' : 'TB';

    nodes = layoutNodes(allNodes, allEdges, direction);
    edges = allEdges;
  }

  /** Solution-level view: Contexts as group containers with direct children only */
  function computeSolutionView(
    graph: IrGraph,
    children: IrNode[],
    contexts: IrNode[],
    sagas: IrNode[],
  ) {
    const allNodes: Node[] = [];
    const allEdges: Edge[] = [];

    // Calculate context sizes based on number of children
    const CTX_PADDING = 30;
    const NODE_W = 200;
    const NODE_H = 80;
    const COLS = 3;
    const CTX_GAP = 100;

    contexts.forEach((ctx, i) => {
      const ctxChildren = getChildren(graph, ctx.id);
      const sagaStepsInCtx = sagas.flatMap(saga =>
        getChildren(graph, saga.id).filter(step => {
          const p = step.metadata.properties.find(([k]) => k === 'ctx');
          return p && p[1] === ctx.name;
        })
      );
      const totalItems = ctxChildren.length + sagaStepsInCtx.length;
      const rows = Math.ceil(totalItems / COLS);
      const ctxW = COLS * (NODE_W + 20) + CTX_PADDING * 2;
      const ctxH = rows * (NODE_H + 20) + 80; // 80 for header

      const x = i * (ctxW + CTX_GAP);

      // Context group node
      allNodes.push({
        id: String(ctx.id),
        type: 'veil',
        position: { x, y: 0 },
        data: {
          label: ctx.name,
          kind: ctx.kind,
          hasChildren: true,
          annotations: ctx.metadata.annotations,
          properties: [],
          isGroup: true,
        },
        style: `width: ${ctxW}px; height: ${ctxH}px;`,
      });

      // Direct children of context (aggregates, ports, services) — NO grandchildren
      ctxChildren.forEach((child, j) => {
        const col = j % COLS;
        const row = Math.floor(j / COLS);
        allNodes.push({
          id: String(child.id),
          type: 'veil',
          position: { x: CTX_PADDING + col * (NODE_W + 20), y: 60 + row * (NODE_H + 20) },
          parentId: String(ctx.id),
          extent: 'parent' as const,
          data: {
            label: child.name,
            kind: child.kind,
            hasChildren: getChildren(graph, child.id).length > 0,
            annotations: child.metadata.annotations,
            properties: child.metadata.properties,
          },
        });
      });

      // Saga steps that belong to this context
      sagaStepsInCtx.forEach((step, j) => {
        const idx = ctxChildren.length + j;
        const col = idx % COLS;
        const row = Math.floor(idx / COLS);
        const saga = sagas.find(s => getChildren(graph, s.id).some(st => st.id === step.id));
        const stepId = `saga-step-${step.id}`;

        allNodes.push({
          id: stepId,
          type: 'veil',
          position: { x: CTX_PADDING + col * (NODE_W + 20), y: 60 + row * (NODE_H + 20) },
          parentId: String(ctx.id),
          extent: 'parent' as const,
          data: {
            label: step.name,
            kind: step.kind,
            hasChildren: getChildren(graph, step.id).length > 0,
            annotations: step.metadata.annotations,
            properties: step.metadata.properties,
            sagaName: saga?.name,
          },
        });
      });
    });

    // Saga sequence edges across contexts
    for (const saga of sagas) {
      const sagaSteps = getChildren(graph, saga.id);
      let prevStepId: string | null = null;
      for (const step of sagaSteps) {
        const stepId = `saga-step-${step.id}`;
        if (prevStepId) {
          allEdges.push({
            id: `saga-edge-${prevStepId}-${stepId}`,
            source: prevStepId,
            target: stepId,
            animated: true,
            style: 'stroke: #dc2626; stroke-width: 2.5; stroke-dasharray: 6 3;',
            label: saga.name,
            labelStyle: 'font-size: 9px; fill: #dc2626;',
          });
        }
        prevStepId = stepId;
      }
    }

    // Adapters below contexts
    const others = children.filter(c => c.kind !== 'Context' && c.kind !== 'Saga');
    const ctxTotalWidth = contexts.length * 800;
    others.forEach((child, i) => {
      allNodes.push({
        id: String(child.id),
        type: 'veil',
        position: { x: i * 250, y: 600 },
        data: {
          label: child.name,
          kind: child.kind,
          hasChildren: getChildren(graph, child.id).length > 0,
          annotations: child.metadata.annotations,
          properties: child.metadata.properties,
        },
      });
    });

    // Implements edges (adapter -> port)
    for (const edge of graph.edges) {
      if (edge.kind === 'Implements') {
        const sourceNode = allNodes.find(n => n.id === String(edge.from));
        const targetNode = allNodes.find(n => n.id === String(edge.to));
        if (sourceNode && targetNode) {
          allEdges.push({
            id: `impl-${edge.from}-${edge.to}`,
            source: sourceNode.id,
            target: targetNode.id,
            animated: false,
            style: getEdgeStyle('Implements'),
          });
        }
      }
    }

    nodes = allNodes;
    edges = allEdges;
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
  {:else}
    <div class="main-layout">
      <Palette contextKind={currentContextKind} />
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
          />
        {/if}
      </div>
    </div>
  {/if}
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
