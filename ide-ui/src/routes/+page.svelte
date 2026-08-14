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
  import CreateConstructMenu, { type CreateItem } from '$lib/CreateConstructMenu.svelte';
  import PropertyEditor from '$lib/PropertyEditor.svelte';
  import DiagnosticsPanel from '$lib/DiagnosticsPanel.svelte';
  import CodePreview from '$lib/CodePreview.svelte';
  import ReviewDock from '$lib/ReviewDock.svelte';
  // CodePreview is embedded in ReviewDock; kept as floating fallback when dock is off.
  import AgentSideRail from '$lib/AgentSideRail.svelte';
  import OutlinePanel from '$lib/OutlinePanel.svelte';
  import DiffPanel from '$lib/DiffPanel.svelte';
  import DevToolbar from '$lib/DevToolbar.svelte';
  import { layoutNodes, layoutByType } from '$lib/layout';
  import { agentPlacement, setAgentPlacement } from '$lib/agentLayout';
  import {
    irGraph,
    currentParent,
    breadcrumbs,
    loading,
    error,
    fetchIr,
    startRevisionWatch,
    drillDown,
    navigateTo,
    navigateUp,
    navigateUpFromFlow,
    getChildren,
    selectedNodeId,
    paletteConfig,
    presentationModel,
    saveEdits,
    availableFiles,
    activeFileName,
    activeFileKind,
    activeProject,
    hubSnapshot,
    selectFile,
    createFile,
    openProject,
    createHubProject,
    currentProjectParam,
    ideApiBase,
    diagnostics,
    viewRevision,
    agentActive,
    embedShellConfig,
    isFlowComposerMode,
    flowLayerParam,
    ensureCodingSession,
  } from '$lib/store';
  import SessionStatus from '$lib/SessionStatus.svelte';
  import { NODE_STYLES, type IrNode, type IrGraph, type NodeKind, type PaletteEntry } from '$lib/types';
  import {
    projectView,
    pickDefaultView,
    viewsForHost,
    irChildren,
    isLogicFlowHost,
    canDrillInto,
    structuralTreeProjection,
    type ViewSpec,
  } from '$lib/presentation';
  import {
    resolveCreateParentSpan,
    uniqueConstructName,
  } from '$lib/createPlacement';
  import { isCriticalNode, countCritical, collectAllLenses, countByLenses, nodeMatchesLenses } from '$lib/lenses';
  import { TreeLayout, FlatLayout, DetailPanel } from '$lib/layouts';
  import type { ProjectResult } from '$lib/presentation';
  import {
    loadOutlineWidth,
    saveOutlineWidth,
    clampOutlineWidth,
    OUTLINE_MIN,
    OUTLINE_MAX,
  } from '$lib/outlineLayout';

  const nodeTypes: NodeTypes = {
    veil: VeilNode as any,
  };

  let nodes = $state.raw<Node[]>([]);
  let edges = $state.raw<Edge[]>([]);
  let nextNodeId = $state(1000);
  /** Outline sidebar width (tree/flat); persisted in localStorage. */
  let outlineWidth = $state(
    typeof window !== 'undefined' ? loadOutlineWidth() : 320
  );
  let outlineResizing = $state(false);

  function startOutlineResize(e: PointerEvent) {
    // Mirror AgentDock: pointer capture + window listeners so drag isn't lost.
    e.preventDefault();
    e.stopPropagation();
    const handle = e.currentTarget as HTMLElement;
    const pointerId = e.pointerId;
    try {
      handle.setPointerCapture(pointerId);
    } catch {
      /* ignore */
    }
    outlineResizing = true;
    document.body.classList.add('outline-resizing');
    const startX = e.clientX;
    const startW = outlineWidth;
    let latest = startW;

    const onMove = (ev: PointerEvent) => {
      if (ev.pointerId !== pointerId) return;
      latest = clampOutlineWidth(startW + (ev.clientX - startX));
      outlineWidth = latest;
    };
    const onUp = (ev: PointerEvent) => {
      if (ev.pointerId !== pointerId) return;
      outlineResizing = false;
      document.body.classList.remove('outline-resizing');
      try {
        handle.releasePointerCapture(pointerId);
      } catch {
        /* ignore */
      }
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onUp);
      outlineWidth = latest;
      saveOutlineWidth(latest);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onUp);
  }
  let flowKey = $state(0);
  let tabs = $state<string[]>([]);
  let activeTab = $state<string | null>(null);
  /** Layer presentation views for current host (LAY-003). */
  let hostViews = $state<ViewSpec[]>([]);
  let activeViewId = $state<string | null>(null);
  let showLayerProvided = $state(false);
  /** LAY-009 / UX-022: filter canvas to presentation lens `critical` + escape diags. */
  let showCriticalOnly = $state(false);
  /** Multi-lens selector: active filter lenses. Empty = show all. */
  let activeLenses = $state<Set<string>>(new Set());
  /** Lens picker popover visibility. */
  let lensPickerOpen = $state(false);
  /** Active projection result for native layout renderers (tree/flat). */
  let currentProjected = $state<ProjectResult | null>(null);
  /** Resolved layout for the current view — drives renderer dispatch. */
  let currentLayout = $state<string>('flat');
  let newProjectName = $state('');
  let creatingProject = $state(false);
  /** Inline “new file” form in the breadcrumb bar. */
  let showNewFile = $state(false);
  let newFileName = $state('');
  let newFileKind = $state<'package' | 'layer'>('package');
  let creatingFile = $state(false);
  // DOM reference for node measurement — ELK needs real rendered sizes
  let graphContainerEl: HTMLElement | null = $state(null);

  /** Mount-time shell (`?mode=flow|reaction` → layer flow composer only). */
  const shell = embedShellConfig();
  const flowLayer = flowLayerParam();

  // Flow mode: force agent on the right so vibe-coding is available.
  if (shell.mode === 'flow' && shell.showAgentRail) {
    setAgentPlacement('right');
  }

  /** Attach-picker: drag from a node handle → choose palette construct. */
  let attachOpen = $state(false);
  let attachSourceId = $state<string | null>(null);
  let connectFromId = $state<string | null>(null);

  async function handleCreateProject() {
    const name = newProjectName.trim();
    if (!name || creatingProject) return;
    creatingProject = true;
    try {
      const info = await createHubProject(name);
      if (info?.name) {
        openProject(info.name);
      } else {
        alert(`Failed to create project "${name}"`);
      }
    } finally {
      creatingProject = false;
    }
  }

  async function handleCreateFile() {
    const name = newFileName.trim();
    if (!name || creatingFile) return;
    creatingFile = true;
    try {
      const result = await createFile({ name, kind: newFileKind });
      if (result?.ok) {
        newFileName = '';
        showNewFile = false;
      } else if (!$error) {
        alert(`Failed to create file "${name}"`);
      }
    } finally {
      creatingFile = false;
    }
  }
  let theme = $state<'dark' | 'light'>(
    (typeof localStorage !== 'undefined' && localStorage.getItem('veil-theme') as 'dark' | 'light') || 'dark'
  );

  function applyTheme(t: 'dark' | 'light') {
    document.documentElement.setAttribute('data-theme', t);
    // Aether components use Tailwind `dark:` variants (class strategy)
    document.documentElement.classList.toggle('dark', t === 'dark');
  }

  function toggleTheme() {
    theme = theme === 'dark' ? 'light' : 'dark';
    applyTheme(theme);
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

  /**
   * LAY-008 / UX-012: create_construct from palette drop or tree Add menu.
   */
  async function createFromPaletteItem(item: {
    kind?: NodeKind | string;
    label: string;
    icon?: string;
    name?: string;
    keyword?: string;
    group?: string;
    dg?: string;
    is_step?: boolean;
  }) {
    const graph = get(irGraph);
    const hostId = get(currentParent);
    if (!graph || hostId == null) {
      alert('Cannot create: no active package context.');
      return;
    }

    const keyword = item.keyword || item.name?.toLowerCase() || 'struct';
    const selId = $selectedNodeId ? Number($selectedNodeId) : null;
    const placement = resolveCreateParentSpan(
      {
        name: item.name,
        keyword,
        label: item.label,
        dg: item.dg,
        group: item.group,
      },
      {
        graph,
        hostId,
        selectedId: Number.isFinite(selId) ? selId : null,
        activeGroup: activeTab,
        activeViewId,
        presentation: get(presentationModel),
      }
    );
    if (!placement) {
      alert('Cannot create: missing parent span.');
      return;
    }

    // Ensure default group exists when placing into a named group that is missing.
    // Flow composer: flat graph on the package body — never auto-create groups.
    let parentSpan = placement.parentSpan;
    const wantGroup = shell.mode === 'flow' ? null : (activeTab || item.dg || item.group);
    if (wantGroup && placement.reason.startsWith('host')) {
      const hasGroup = graph.nodes.some(
        (n) =>
          n.kind === 'Group' &&
          n.name === wantGroup &&
          n.metadata.parent === hostId
      );
      if (!hasGroup) {
        const host = graph.nodes.find((n) => n.id === hostId);
        if (host) {
          const okGroup = await saveEdits([
            {
              op: 'create_construct',
              parent_span: host.span.start,
              keyword: 'group',
              name: wantGroup,
            },
          ]);
          if (okGroup) {
            const g2 = get(irGraph);
            const gn = g2?.nodes.find(
              (n) =>
                n.kind === 'Group' &&
                n.name === wantGroup &&
                n.metadata.parent === hostId
            );
            if (gn) parentSpan = gn.span.start;
          }
        }
      }
    }

    const baseName = (item.name || item.label || 'New').replace(/\s+/g, '');
    const name = uniqueConstructName(graph, baseName, hostId);

    // Step-type items in flow mode: use create_step targeting the fn body.
    const hostNode = graph.nodes.find((n) => n.id === hostId);
    const isFlowParent = hostNode && (hostNode.kind === 'Flow' || hostNode.kind === 'Service' || hostNode.kind === 'Orchestrator');
    if (item.is_step && isFlowParent) {
      const ok = await saveEdits([
        {
          op: 'create_step',
          parent_span: hostNode.span.start,
          keyword: item.keyword || item.name?.toLowerCase() || 'step',
          name,
          fields: [],
        },
      ]);
      if (!ok) return;
      const fresh = get(irGraph);
      if (fresh) await computeView(fresh, get(currentParent), get(paletteConfig));
      flowKey += 1;
      return;
    }

    const ok = await saveEdits([
      {
        op: 'create_construct',
        parent_span: parentSpan,
        keyword,
        name,
      },
    ]);
    if (!ok) {
      // saveError store already set
      return;
    }
    const fresh = get(irGraph);
    if (fresh) await computeView(fresh, get(currentParent), get(paletteConfig));
    flowKey += 1;
  }

  /** Palette drop → same create path as tree Add menu. */
  async function handleDrop(event: DragEvent) {
    event.preventDefault();
    if (!event.dataTransfer) return;

    const data = event.dataTransfer.getData('application/veil-node');
    if (!data) return;

    const item = JSON.parse(data) as CreateItem;
    await createFromPaletteItem(item);
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
        const freshRes = await fetch(`${ideApiBase()}/ir`);
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
    // Apply saved theme on mount (veil tokens + Aether dark: class)
    applyTheme(theme);

    // Parent drill / file switch → recompute canvas.
    // Skip parent=null (used only as a force-refresh sentinel before set to root).
    const unsubParent = currentParent.subscribe((parent) => {
      if (parent == null) return;
      const graph = get(irGraph);
      if (!graph) return;
      const palette = get(paletteConfig);
      void computeView(graph, parent, palette);
    });

    // viewRevision bumps after every successful load so computeView runs even
    // when Solution node id is unchanged across packages (always id 1).
    const unsubRev = viewRevision.subscribe((rev) => {
      if (rev === 0) return;
      const graph = get(irGraph);
      const parent = get(currentParent);
      if (!graph || parent == null) return;
      void computeView(graph, parent, get(paletteConfig));
    });

    void fetchIr();
    const stopSse = startRevisionWatch();
    // Durable coding session (S3/DDB) for this project
    const proj = currentProjectParam();
    if (proj) void ensureCodingSession(proj);

    return () => {
      unsubParent();
      unsubRev();
      stopSse();
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

  /** UX-024: summary lines for step body actions (max N). */
  const BODY_PREVIEW_MAX = 4;

  function bodyPreviewFor(
    graph: IrGraph,
    child: IrNode,
    childChildren: IrNode[]
  ): { lines: { text: string; keyword: string | null }[]; empty: boolean; more: number } {
    if (child.kind !== 'Step' && child.kind !== 'ErrorBoundary') {
      return { lines: [], empty: false, more: 0 };
    }
    const actions = childChildren.filter((c) => c.kind === 'Action');
    const subBlocks = childChildren.filter(
      (c) =>
        c.kind === 'Step' && c.metadata.annotations.includes('sub_block')
    );
    const lines: { text: string; keyword: string | null }[] = [];
    for (const a of actions) {
      const sk = a.metadata.subkind;
      const msg = a.metadata.properties.find(([k]) => k === 'message')?.[1];
      let text = a.name;
      if (sk === 'guard' && msg) text = `${a.name} — "${msg}"`;
      lines.push({ text, keyword: sk });
    }
    for (const sb of subBlocks) {
      const nActs = getChildren(graph, sb.id).filter((c) => c.kind === 'Action').length;
      lines.push({
        text: nActs > 0 ? `${sb.name} (${nActs})` : `${sb.name} (empty)`,
        keyword: sb.metadata.subkind ?? sb.name,
      });
    }
    const total = lines.length;
    return {
      lines: lines.slice(0, BODY_PREVIEW_MAX),
      empty: total === 0,
      more: Math.max(0, total - BODY_PREVIEW_MAX),
    };
  }

  /** Map IR nodes to SvelteFlow nodes (shared by presentation + legacy paths). */
  function toFlowNodes(
    graph: IrGraph,
    items: IrNode[],
    visibleIds: Set<number>
  ): Node[] {
    const pres = get(presentationModel);
    const diags = get(diagnostics);
    let list = items;
    if (showCriticalOnly) {
      list = items.filter((c) => isCriticalNode(c, pres, diags));
    }
    if (activeLenses.size > 0) {
      list = list.filter((c) => nodeMatchesLenses(c, pres, diags, activeLenses));
    }
    return list.map((child) => {
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
      // Steps: show body preview on card; drill only into non-Action structure
      // (match arms, compensate sub-blocks). Actions open via VEIL source pane.
      const bodyPrev = bodyPreviewFor(graph, child, childChildren);
      if (child.kind === 'Step') {
        hasChildren = childChildren.some((c) => c.kind !== 'Action');
      }
      // Routing: Calls edges from this step's actions (UX-026)
      const routing = childChildren
        .filter((c) => c.kind === 'Action')
        .flatMap((a) =>
          graph.edges
            .filter((e) => e.from === a.id && e.kind === 'Calls')
            .map((e) => {
              const t = graph.nodes.find((n) => n.id === e.to);
              return t?.name ?? String(e.to);
            })
        );
      const critical = isCriticalNode(child, pres, diags);
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
          doc: child.metadata.doc ?? null,
          inlineChildren,
          refs,
          critical,
          bodyPreview: bodyPrev.lines,
          bodyEmpty: bodyPrev.empty,
          bodyMore: bodyPrev.more,
          routingTargets: [...new Set(routing)],
        },
      };
    });
  }

  function criticalCountLabel(): string {
    const g = get(irGraph);
    if (!g) return '0 critical';
    const n = countCritical(g, get(presentationModel), get(diagnostics));
    return `${n} critical`;
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

  /** Apply native tree/flat projection (no SvelteFlow). */
  function applyNativeProjection(projected: ProjectResult) {
    currentProjected = projected;
    currentLayout = projected.layout === 'flat' ? 'flat' : 'tree';
    nodes = [];
    edges = [];
  }

  /** Order flow-graph peers: Inputs → body (steps/actions) → Return last. */
  function flowSequenceOrder(n: IrNode): number {
    switch (n.kind) {
      case 'Inputs':
        return 0;
      case 'Step':
      case 'Action':
      case 'ParallelGateway':
      case 'MatchDecision':
      case 'ErrorBoundary':
        return 1;
      case 'Return':
        return 9;
      default:
        return 5;
    }
  }

  function flowSeqProp(n: IrNode): number | null {
    const raw = n.metadata.properties.find(([k]) => k === 'seq')?.[1];
    if (raw == null) return null;
    const v = Number(raw);
    return Number.isFinite(v) ? v : null;
  }

  function sortFlowSequence(kids: IrNode[]): IrNode[] {
    return [...kids].sort((a, b) => {
      const tier = flowSequenceOrder(a) - flowSequenceOrder(b);
      if (tier !== 0) return tier;
      // Method/step body Actions: honor emission order (seq), not alphabetical names
      const sa = flowSeqProp(a);
      const sb = flowSeqProp(b);
      if (sa != null && sb != null && sa !== sb) return sa - sb;
      if (sa != null && sb == null) return -1;
      if (sa == null && sb != null) return 1;
      return a.span.start - b.span.start || a.id - b.id || a.name.localeCompare(b.name);
    });
  }

  /** SvelteFlow graph for a logic-flow host (steps / sequence). */
  async function applyLogicFlowGraph(graph: IrGraph, hostId: number) {
    let kids = irChildren(graph, hostId);
    if (!showLayerProvided) {
      kids = kids.filter((c) => !c.metadata.annotations.includes('layer-provided'));
    }
    // Sequence peers: steps/gateways/returns + Action bodies (method/fn statements)
    kids = kids.filter((c) =>
      [
        'Inputs',
        'Step',
        'Action',
        'ParallelGateway',
        'MatchDecision',
        'ErrorBoundary',
        'Return',
      ].includes(c.kind)
    );
    kids = sortFlowSequence(kids);

    currentProjected = null;
    currentLayout = 'flow';
    tabs = [];
    activeTab = null;
    hostViews = [];
    const itemIds = new Set(kids.map((c) => c.id));
    let flowNodes = toFlowNodes(graph, kids, itemIds);
    let flowEdges = edgesAmong(graph, itemIds);

    // Synthesize SequenceFlow chain when IR lacks edges (older graphs / missing last→Return)
    for (let i = 0; i < kids.length - 1; i++) {
      const a = kids[i];
      const b = kids[i + 1];
      const exists = flowEdges.some(
        (e) => e.source === String(a.id) && e.target === String(b.id)
      );
      if (!exists) {
        flowEdges.push({
          id: `seq-synth-${a.id}-${b.id}`,
          source: String(a.id),
          target: String(b.id),
          animated: true,
          style: getEdgeStyle('SequenceFlow'),
          label: '',
          labelStyle: 'font-size: 10px; fill: var(--veil-text-dim);',
        });
      }
    }

    // Resolve on:label routing on Step nodes
    for (const node of kids) {
      if (node.kind !== 'Step') continue;
      for (const [key, value] of node.metadata.properties) {
        if (!key.startsWith('on:')) continue;
        const label = key.slice(3);
        const targetNode = kids.find((n) => n.kind === 'Step' && n.name === value);
        if (targetNode && itemIds.has(targetNode.id)) {
          flowEdges = flowEdges.filter(
            (e) => !(e.source === String(node.id) && e.id?.startsWith('e-'))
          );
          flowEdges.push({
            id: `route-${node.id}-${targetNode.id}-${label}`,
            source: String(node.id),
            target: String(targetNode.id),
            animated: false,
            style: `stroke: var(--node-color, ${node.metadata.properties.find(([k]) => k === 'color')?.[1] || '#737373'}); stroke-width: 2;`,
            label,
            labelStyle: 'font-size: 11px; fill: var(--veil-text); font-weight: 600;',
          });
        }
      }
    }
    nodes = await layoutNodes(flowNodes, flowEdges, 'LR', graphContainerEl);
    edges = flowEdges;
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

    // ─── SvelteFlow only for hosts with real control-flow bodies ───────
    if (parentNode && isLogicFlowHost(graph, parentNode)) {
      await applyLogicFlowGraph(graph, parentNode.id);
      return;
    }

    // ─── Package / solution root: native tree (modules, free fns, …) ───
    if (isSolutionLevel) {
      hostViews = [];
      activeViewId = null;
      tabs = [];
      activeTab = null;
      const projected = structuralTreeProjection(graph, parentId, {
        hideLayerProvided: !showLayerProvided,
        roots: children,
      });
      applyNativeProjection(projected);
      return;
    }

    // ─── LAY-003: layer presentation views ─────────────────────────────
    const pres = get(presentationModel);
    const hostName = parentNode?.metadata.subkind ?? null;
    const views = viewsForHost(pres, hostName);
    hostViews = views;

    if (views.length > 0 && parentId != null) {
      const hostDto = hostName && pres ? pres.hosts[hostName] : undefined;
      // Returning from flow graph: prefer default view (domain model tree)
      // rather than a stale tab/view from before drill-in.
      if (!activeViewId || !views.some((v) => v.id === activeViewId)) {
        activeViewId = pickDefaultView(hostDto, views);
      }
      // Module / Context hosts: always land on default tree when coming from flow
      if (
        parentNode &&
        (parentNode.kind === 'Module' || parentNode.kind === 'Solution') &&
        hostDto
      ) {
        const def = pickDefaultView(hostDto, views);
        if (def) activeViewId = def;
        activeTab = null;
      }
      const view = views.find((v) => v.id === activeViewId) ?? views[0];
      const projected = projectView(graph, parentId, view, {
        hideLayerProvided: !showLayerProvided,
      });

      // Tabs = optional layer filters; body is group-aware structural tree
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
        const groupRoots: IrNode[] = [];
        if (groupNode) {
          let gc = irChildren(graph, groupNode.id);
          if (!showLayerProvided) {
            gc = gc.filter((c) => !c.metadata.annotations.includes('layer-provided'));
          }
          groupRoots.push(...gc);
          const nonGroup = children.filter((c) => c.kind !== 'Group');
          groupRoots.push(...nonGroup);
        }
        const tree = structuralTreeProjection(graph, groupNode?.id ?? parentId, {
          hideLayerProvided: !showLayerProvided,
          roots: groupRoots.length > 0 ? groupRoots : undefined,
        });
        applyNativeProjection(tree);
        return;
      }

      tabs = [];
      activeTab = null;

      // Explicit flow layout from layer — only if host actually has flow logic
      if (projected.layout === 'flow') {
        if (parentNode && isLogicFlowHost(graph, parentNode)) {
          await applyLogicFlowGraph(graph, parentId);
        } else {
          applyNativeProjection(
            structuralTreeProjection(graph, parentId, {
              hideLayerProvided: !showLayerProvided,
            })
          );
        }
        return;
      }

      // tree | flat | unknown — single structural outline with Group folders.
      // Nest-rule model views flattened groups and encouraged tree→tree drill;
      // expand/collapse in place is the default for structural hosts.
      applyNativeProjection(
        structuralTreeProjection(graph, parentId, {
          hideLayerProvided: !showLayerProvided,
        })
      );
      return;
    }

    // ─── Fallback: structural tree (+ optional group tabs) ─────────────
    hostViews = [];
    activeViewId = null;

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
      const groupRoots: IrNode[] = [];
      if (activeGroup) {
        groupRoots.push(...getChildren(graph, activeGroup.id));
        groupRoots.push(...children.filter((c) => c.kind !== 'Group'));
      }
      applyNativeProjection(
        structuralTreeProjection(graph, activeGroup?.id ?? parentId, {
          hideLayerProvided: !showLayerProvided,
          roots: groupRoots,
        })
      );
      return;
    }

    tabs = [];
    activeTab = null;
    applyNativeProjection(
      structuralTreeProjection(graph, parentId, {
        hideLayerProvided: !showLayerProvided,
        roots: children,
      })
    );
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

    // Always update selection (opens property panel)
    selectedNodeId.set(node.id);

    // Show reference edges for the selected node, hide others
    updateReferenceEdges(graph, node.id);

    // Double-click: only open control-flow graphs (not tree→tree for Module/Group)
    if (!shell.allowDrillDown) return;
    if (irNode && event instanceof MouseEvent && event.detail === 2) {
      if (canDrillInto(graph, irNode)) {
        drillDown(irNode);
        selectedNodeId.set(null);
      }
    }
  }

  /** Outline drill: only logic-flow hosts (fn/method with steps). */
  function handleOutlineDrill(node: IrNode) {
    const graph = get(irGraph);
    if (!graph || !shell.allowDrillDown) return;
    if (!canDrillInto(graph, node)) return;
    drillDown(node);
    selectedNodeId.set(null);
  }

  function handleConnectStart({ nodeId }: { nodeId: string | null }) {
    connectFromId = nodeId;
  }

  function handleConnectEnd(event: MouseEvent | TouchEvent) {
    if (!shell.attachPickerOnConnect || !connectFromId) {
      connectFromId = null;
      return;
    }
    // Dropped on another node → onconnect already fired; only open picker when
    // the gesture ends on empty canvas (no valid target).
    const t = event.target as HTMLElement | null;
    const onNode = t?.closest?.('.svelte-flow__node');
    if (onNode) {
      connectFromId = null;
      return;
    }
    attachSourceId = connectFromId;
    attachOpen = true;
    connectFromId = null;
  }

  function closeAttachPicker() {
    attachOpen = false;
    attachSourceId = null;
  }

  async function attachPaletteItem(item: PaletteEntry) {
    const graph = get(irGraph);
    const hostId = get(currentParent);
    if (!graph || hostId == null) {
      alert('Cannot create: no package context.');
      closeAttachPicker();
      return;
    }
    const keyword = item.keyword || item.name?.toLowerCase() || 'struct';
    const placement = resolveCreateParentSpan(
      {
        name: item.name,
        keyword,
        label: item.label,
        dg: item.dg,
        group: item.group,
      },
      {
        graph,
        hostId,
        selectedId: attachSourceId ? Number(attachSourceId) : null,
        activeGroup: activeTab,
        activeViewId,
        presentation: get(presentationModel),
      }
    );
    if (!placement) {
      alert('Cannot create: missing parent span.');
      closeAttachPicker();
      return;
    }
    const baseName = (item.name || item.label || 'New').replace(/\s+/g, '');
    const name = uniqueConstructName(graph, baseName, hostId);
    const ok = await saveEdits([
      {
        op: 'create_construct',
        parent_span: placement.parentSpan,
        keyword,
        name,
      },
    ]);
    closeAttachPicker();
    if (!ok) return;
    const fresh = get(irGraph);
    if (fresh) await computeView(fresh, get(currentParent), get(paletteConfig));
    flowKey += 1;
  }

  /** Add/remove reference edges + focus related domain nodes when selected. */
  function updateReferenceEdges(graph: IrGraph, selectedId: string | null) {
    // Remove any existing reference edges / clear focus styling
    edges = edges.filter((e) => !String(e.id).startsWith('ref-'));
    nodes = nodes.map((n) => ({
      ...n,
      style: {
        ...(typeof n.style === 'object' && n.style ? n.style : {}),
        opacity: 1,
      },
      class: String(n.class ?? '')
        .split(/\s+/)
        .filter((c) => c && c !== 'focus-related' && c !== 'focus-dim')
        .join(' '),
    }));

    if (!selectedId) return;

    // Build a position lookup from visible nodes
    const nodePositions = new Map<string, { x: number; y: number }>();
    for (const n of nodes) {
      nodePositions.set(n.id, n.position);
    }

    // Find reference edges that touch the selected node
    const nodeId = Number(selectedId);
    const visibleIds = new Set(nodes.map((n) => Number(n.id)));

    // Related set: selected + References neighbors + nest owns edges on canvas
    const related = new Set<number>([nodeId]);
    for (const e of graph.edges) {
      if (e.kind !== 'References') continue;
      if (e.from === nodeId) related.add(e.to);
      if (e.to === nodeId) related.add(e.from);
    }
    for (const e of edges) {
      const id = String(e.id);
      if (!id.startsWith('nest-') && !id.startsWith('bucket-')) continue;
      const s = Number(e.source);
      const t = Number(e.target);
      if (s === nodeId || t === nodeId) {
        related.add(s);
        related.add(t);
      }
    }

    // Dim unrelated nodes so the domain stays visible but focus is clear
    if (related.size > 1 || nodes.length > 1) {
      nodes = nodes.map((n) => {
        const id = Number(n.id);
        const isRelated = related.has(id) || n.id === selectedId;
        return {
          ...n,
          style: {
            ...(typeof n.style === 'object' && n.style ? n.style : {}),
            opacity: isRelated ? 1 : 0.28,
            transition: 'opacity 0.15s ease',
          },
          class: [
            String(n.class ?? '')
              .split(/\s+/)
              .filter((c) => c && c !== 'focus-related' && c !== 'focus-dim')
              .join(' '),
            isRelated ? 'focus-related' : 'focus-dim',
          ]
            .filter(Boolean)
            .join(' '),
        };
      });
    }

    const refEdges: Edge[] = graph.edges
      .filter((e) => e.kind === 'References')
      .filter((e) => e.from === nodeId || e.to === nodeId)
      .filter((e) => visibleIds.has(e.from) && visibleIds.has(e.to))
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
          animated: true,
          style: getEdgeStyle('References'),
          label: 'ref',
          labelStyle: 'font-size: 9px; fill: #60a5fa;',
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

    // Esc leaves flow graph → module/package outline (skips group buckets)
    if (event.key === 'Escape' && currentLayout === 'flow') {
      event.preventDefault();
      navigateUpFromFlow();
      return;
    }

    // Enter: open control-flow graph only (same as double-click)
    if (event.key === 'Enter' && $selectedNodeId) {
      const graph = get(irGraph);
      if (!graph) return;
      const irNode = graph.nodes.find(n => n.id === Number($selectedNodeId));
      if (irNode) handleOutlineDrill(irNode);
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

  /** UX-013: freeform connect is local-only — edges are derived from IR on reload. */
  function handleConnect(connection: { source: string; target: string }) {
    const newEdge: Edge = {
      id: `local-${connection.source}-${connection.target}-${Date.now()}`,
      source: connection.source,
      target: connection.target,
      animated: true,
      style: 'stroke: #f59e0b; stroke-width: 2; stroke-dasharray: 4 3;',
      label: 'local only',
      labelStyle: 'font-size: 9px; fill: #f59e0b;',
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

<svelte:window onkeydown={handleKeyDown} onpointerdown={() => { if (lensPickerOpen) lensPickerOpen = false; }} />

<div
  class="viewer-container"
  class:viewer-container--flow={shell.mode === 'flow'}
  data-veil-shell={shell.mode}
>
  {#if shell.showTopBar}
    <!-- Full dual-loop top bar (hidden in flow/reaction composer mode) -->
    <div class="top-bar">
      <div class="breadcrumbs">
        {#if $hubSnapshot?.multi && $hubSnapshot.projects.length > 0}
          <select
            class="file-selector project-switcher"
            title="Switch product project"
            value={currentProjectParam() ?? ''}
            onchange={(e) => {
              const name = (e.currentTarget as HTMLSelectElement).value;
              if (name) openProject(name);
            }}
          >
            <option value="" disabled={!!currentProjectParam()}>Projects…</option>
            {#each $hubSnapshot.projects as p}
              <option value={p.name}>{p.name}</option>
            {/each}
          </select>
          <span class="breadcrumb-sep">›</span>
        {:else if $activeProject?.name}
          <span class="project-badge" title={$activeProject.path ?? ''}>{$activeProject.name}</span>
          <span class="breadcrumb-sep">›</span>
        {/if}
        {#if $availableFiles.length > 0}
          <select
            class="file-selector"
            title="Switch package or layer"
            value={$availableFiles.findIndex(f => f.active)}
            onchange={(e) => {
              const idx = Number((e.currentTarget as HTMLSelectElement).value);
              if (!Number.isFinite(idx)) return;
              void selectFile(idx);
            }}
          >
            {#each $availableFiles as file}
              <option value={file.index}
                >{file.kind === 'layer' ? '📐 ' : file.kind === 'stub' ? '📎 ' : ''}{file.name}{file.active
                  ? ' ●'
                  : ''}{file.adapts ? ` ↳ ${file.adapts}` : ''}</option
              >
            {/each}
          </select>
        {/if}
        <button
          type="button"
          class="add-file-btn"
          title="Add package or layer file to this project"
          onclick={() => {
            showNewFile = !showNewFile;
            if (showNewFile) newFileName = '';
          }}
        >
          +
        </button>
        {#if showNewFile}
          <div class="new-file-form" role="group" aria-label="New file">
            <select
              class="file-selector kind-select"
              bind:value={newFileKind}
              title="File kind"
            >
              <option value="package">package (.veil)</option>
              <option value="layer">layer (.layer)</option>
            </select>
            <input
              class="new-file-input"
              placeholder={newFileKind === 'layer' ? 'my_layer' : 'MyPackage'}
              bind:value={newFileName}
              onkeydown={(e) => {
                if (e.key === 'Enter') void handleCreateFile();
                if (e.key === 'Escape') showNewFile = false;
              }}
            />
            <button
              type="button"
              class="retry-btn new-file-go"
              disabled={creatingFile || !newFileName.trim()}
              onclick={() => void handleCreateFile()}
            >
              {creatingFile ? '…' : 'Add'}
            </button>
          </div>
        {/if}
        {#if $availableFiles.length > 0 || showNewFile}
          <span class="breadcrumb-sep">›</span>
        {/if}
        {#if $activeFileKind === 'layer'}
          <span class="kind-badge" title="Language designer mode">layer</span>
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
      {#if shell.showOutline}
        <OutlinePanel />
      {/if}
      {#if shell.showDiff}
        <DiffPanel />
      {/if}
      {#if shell.showInfraToggle}
        <label class="layer-toggle" title="Layer-provided constructs (default: hidden). When shown, dimmed and labeled infra.">
          <input type="checkbox" bind:checked={showLayerProvided} onchange={() => { const g = get(irGraph); const p = get(currentParent); if (g) computeView(g, p); }} />
          <span>Show infrastructure</span>
        </label>
      {/if}
      {#if shell.showCriticalToggle}
        {@const allLenses = collectAllLenses(get(presentationModel))}
        {#if allLenses.length > 0}
          <div class="lens-picker-wrapper" onpointerdown={(e) => e.stopPropagation()}>
            <button
              class="lens-picker-btn"
              class:active={activeLenses.size > 0}
              onclick={() => { lensPickerOpen = !lensPickerOpen; }}
              title="Filter by review lens (LAY-009)"
            >
              🔍 Lenses
              {#if activeLenses.size > 0}
                <span class="lens-active-count">{activeLenses.size}</span>
              {/if}
            </button>
            {#if lensPickerOpen}
              <div class="lens-picker-popover" role="menu">
                {#each allLenses as lens}
                  {@const active = activeLenses.has(lens)}
                  {@const count = countByLenses(get(irGraph) ?? { nodes: [], edges: [], next_id: 0 }, get(presentationModel), get(diagnostics), new Set([lens]))}
                  <label class="lens-picker-item" role="menuitemcheckbox" aria-checked={active}>
                    <input
                      type="checkbox"
                      checked={active}
                      onchange={() => {
                        const next = new Set(activeLenses);
                        if (active) next.delete(lens);
                        else next.add(lens);
                        activeLenses = next;
                        // Sync legacy showCriticalOnly for backward compat
                        showCriticalOnly = next.has('critical');
                        const g = get(irGraph); const p = get(currentParent);
                        if (g) computeView(g, p, get(paletteConfig));
                      }}
                    />
                    <span class="lens-name">{lens}</span>
                    <span class="lens-count">{count}</span>
                  </label>
                {/each}
                {#if activeLenses.size > 0}
                  <button
                    class="lens-clear-btn"
                    onclick={() => {
                      activeLenses = new Set();
                      showCriticalOnly = false;
                      lensPickerOpen = false;
                      const g = get(irGraph); const p = get(currentParent);
                      if (g) computeView(g, p, get(paletteConfig));
                    }}
                  >
                    Clear all
                  </button>
                {/if}
              </div>
            {/if}
          </div>
        {:else}
          <!-- Fallback: simple critical toggle when no lenses declared -->
          <label class="layer-toggle" title="Layer lens critical + escape/error diagnostics (LAY-009)">
            <input type="checkbox" bind:checked={showCriticalOnly} onchange={() => { const g = get(irGraph); const p = get(currentParent); if (g) computeView(g, p, get(paletteConfig)); }} />
            <span>Critical only</span>
            <span class="critical-count">{criticalCountLabel()}</span>
          </label>
        {/if}
      {/if}
      <SessionStatus />
      {#if shell.showThemeToggle}
        <button class="theme-toggle" onclick={toggleTheme} title="Toggle light/dark mode">
          {theme === 'dark' ? '☀️' : '🌙'}
        </button>
      {/if}
    </div>
  {/if}

  {#if $loading}
    <div class="status-overlay">
      <div class="pulse-ring"></div>
      <p>Loading...</p>
    </div>
  {:else if $hubSnapshot?.multi && !currentProjectParam()}
    <div class="status-overlay hub-picker">
      <p class="error-title">Select a project</p>
      <p class="error-msg">
        Multi-project host · hub
        <code>{$hubSnapshot.projects_dir || '…'}</code>
      </p>
      {#if $hubSnapshot.projects.length === 0}
        <p class="error-hint">No products yet. Create one below or run <code>veil projects create my-app</code>.</p>
      {:else}
        <ul class="hub-list">
          {#each $hubSnapshot.projects as p}
            <li>
              <button class="retry-btn" type="button" onclick={() => openProject(p.name)}>
                Open {p.name}
              </button>
              <span class="hub-meta">{p.package_count ?? 0} pkg · {p.is_git ? 'git' : 'no-git'}</span>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="hub-create">
        <input
          class="hub-input"
          placeholder="new-project-name"
          bind:value={newProjectName}
          onkeydown={(e) => {
            if (e.key === 'Enter') void handleCreateProject();
          }}
        />
        <button class="retry-btn" type="button" disabled={creatingProject} onclick={() => void handleCreateProject()}>
          {creatingProject ? 'Creating…' : 'Create project'}
        </button>
      </div>
      <p class="error-hint">
        API: <code>veil-runtime</code> (:8080) or <code>veil serve --multi</code> (:3001).
        Override host with <code>?api=http://localhost:3001</code>.
      </p>
    </div>
  {:else if $error}
    <div class="status-overlay error">
      <p class="error-title">⚠️ Connection Error</p>
      <p class="error-msg">{$error}</p>
      <p class="error-hint">
        Run: <code>make runtime-serve</code> or <code>veil serve --multi -p 3001</code>
        + viewer <code>:5173</code>
        (or single project: <code>make serve PROJECT=…</code>)
      </p>
      <button class="retry-btn" onclick={() => void fetchIr()}>Retry</button>
    </div>
  <!-- Scope panel — shows variables available at current level -->
  {:else}
    {#if shell.showScopeBar && scopeVars.length > 0}
      <div class="scope-bar">
        <span class="scope-label">Scope:</span>
        {#each scopeVars as v}
          <span class="scope-var">{v}</span>
        {/each}
      </div>
    {/if}
    {#if shell.showDevToolbar}
      <DevToolbar />
    {/if}
    {#if $agentActive && shell.showAgentRail}
      <div class="agent-activity-bar">
        <span class="agent-activity-dot"></span>
        <span class="agent-activity-text">Agent editing…</span>
      </div>
    {/if}
    <div
      class="main-layout"
      class:main-layout--flow={shell.mode === 'flow'}
      class:main-layout--structural={currentLayout === 'tree' || currentLayout === 'flat'}
    >
      <!-- Constructs palette only for SvelteFlow (drag onto canvas). Tree/flat use Add menu. -->
      {#if currentLayout !== 'tree' && currentLayout !== 'flat'}
        <Palette contextKind={currentContextKind} contextKindCore={currentContextKindCore} activeGroup={activeTab} />
      {/if}
      {#if shell.showAgentRail && $agentPlacement === 'left'}
        <AgentSideRail side="left" />
      {/if}
      <div class="graph-wrapper">
        {#if shell.showViewBar && hostViews.length > 1}
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
        {#if shell.showGroupTabs && tabs.length > 0}
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
        {#if currentLayout === 'tree' && currentProjected && $irGraph}
          <!-- Native tree renderer -->
          <div class="native-layout-container" class:resizing={outlineResizing}>
            <div
              class="native-layout-sidebar"
              style="width: {outlineWidth}px; flex: 0 0 {outlineWidth}px; min-width: {OUTLINE_MIN}px; max-width: {OUTLINE_MAX}px;"
            >
              <div class="tree-toolbar">
                <span class="tree-toolbar-title">Outline</span>
                <CreateConstructMenu
                  contextKind={currentContextKind}
                  contextKindCore={currentContextKindCore}
                  activeGroup={activeTab}
                  onCreate={(item) => createFromPaletteItem(item)}
                />
              </div>
              <TreeLayout
                projected={currentProjected}
                graph={$irGraph}
                presentationModel={$presentationModel}
                onDrillDown={handleOutlineDrill}
              />
            </div>
            <div
              class="outline-resize-handle"
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize outline pane"
              aria-valuenow={outlineWidth}
              aria-valuemin={OUTLINE_MIN}
              aria-valuemax={OUTLINE_MAX}
              title="Drag to resize outline"
              onpointerdown={startOutlineResize}
            ></div>
            <div class="native-layout-detail">
              <DetailPanel
                graph={$irGraph}
                presentationModel={$presentationModel}
              />
            </div>
          </div>
        {:else if currentLayout === 'flat' && currentProjected && $irGraph}
          <!-- Native flat renderer -->
          <div class="native-layout-container" class:resizing={outlineResizing}>
            <div
              class="native-layout-sidebar"
              style="width: {outlineWidth}px; flex: 0 0 {outlineWidth}px; min-width: {OUTLINE_MIN}px; max-width: {OUTLINE_MAX}px;"
            >
              <div class="tree-toolbar">
                <span class="tree-toolbar-title">Constructs</span>
                <CreateConstructMenu
                  contextKind={currentContextKind}
                  contextKindCore={currentContextKindCore}
                  activeGroup={activeTab}
                  onCreate={(item) => createFromPaletteItem(item)}
                />
              </div>
              <FlatLayout
                projected={currentProjected}
                graph={$irGraph}
                presentationModel={$presentationModel}
                onDrillDown={handleOutlineDrill}
              />
            </div>
            <div
              class="outline-resize-handle"
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize outline pane"
              aria-valuenow={outlineWidth}
              aria-valuemin={OUTLINE_MIN}
              aria-valuemax={OUTLINE_MAX}
              title="Drag to resize outline"
              onpointerdown={startOutlineResize}
            ></div>
            <div class="native-layout-detail">
              <DetailPanel
                graph={$irGraph}
                presentationModel={$presentationModel}
              />
            </div>
          </div>
        {:else}
          <!-- Flow graph: always offer an obvious exit back to the parent tree host -->
          {@const flowHost = $irGraph?.nodes.find((n) => n.id === $currentParent)}
          {@const flowBackLabel = (() => {
            // Label the nearest non-Group ancestor (what Back will restore)
            let pid = flowHost?.metadata.parent ?? null;
            const nodes = $irGraph?.nodes ?? [];
            while (pid != null) {
              const p = nodes.find((n) => n.id === pid);
              if (!p) break;
              if (p.kind !== 'Group') return p.name;
              pid = p.metadata.parent;
            }
            return 'Outline';
          })()}
          <div class="flow-nav-bar" role="navigation" aria-label="Flow view navigation">
            <button
              type="button"
              class="flow-back-btn"
              onclick={() => {
                if (!navigateUpFromFlow()) {
                  const sol = $irGraph?.nodes.find((n) => n.kind === 'Solution');
                  if (sol) navigateTo(sol.id);
                }
              }}
              title="Leave flow graph and return to full outline"
            >
              ← {flowBackLabel}
            </button>
            <span class="flow-nav-sep" aria-hidden="true">/</span>
            <span class="flow-nav-here" title={flowHost?.metadata.subkind ?? flowHost?.kind ?? ''}>
              {flowHost?.name ?? 'Flow'}
            </span>
            {#if flowHost?.metadata.subkind}
              <span class="flow-nav-kind">{flowHost.metadata.subkind}</span>
            {/if}
            <span class="flow-nav-hint">Flow graph · Esc to leave</span>
          </div>
        {#if shell.mode === 'flow'}
          {@const parentNode = $irGraph?.nodes.find((n) => n.id === $currentParent)}
          {@const fnParams = parentNode?.metadata.properties.find(([k]) => k === 'params')?.[1] ?? ''}
          {@const fnReturn = parentNode?.metadata.properties.find(([k]) => k === 'returns')?.[1] ?? ''}
          <div class="fn-signature-bar">
            <span class="fn-kw">fn</span>
            <span class="fn-name">{parentNode?.name ?? 'run'}</span>
            <span class="fn-params">({fnParams})</span>
            {#if fnReturn}
              <span class="fn-arrow">→</span>
              <span class="fn-return">{fnReturn}</span>
            {/if}
          </div>
        {/if}
        {#if shell.showDiagnostics}
          <!-- Offset below flow-nav so the badge never covers ← Back -->
          <DiagnosticsPanel offsetTop={currentLayout === 'flow' ? 48 : 12} />
        {/if}
        
        {#key flowKey}
        <SvelteFlow
          bind:nodes
          bind:edges
          {nodeTypes}
          fitView
          fitViewOptions={{ maxZoom: 1, padding: 0.2 }}
          onnodeclick={handleNodeClick}
          onconnect={handleConnect}
          onconnectstart={(_e, params) => handleConnectStart({ nodeId: params.nodeId ?? null })}
          onconnectend={handleConnectEnd}
          onpaneclick={handlePaneClick}
          colorMode={theme}
        >
          <Background variant={BackgroundVariant.Dots} gap={20} size={1} />
          {#if shell.showFlowControls}
            <Controls />
          {/if}
          {#if shell.showMiniMap}
            <MiniMap />
          {/if}
        </SvelteFlow>
        {/key}

        {#if selectedNode}
          <PropertyEditor
            node={selectedNode}
            onUpdate={updateNodeData}
            onClose={() => selectedNodeId.set(null)}
            
          />
        {/if}
        {/if}
      </div>
      </div>
      {#if shell.showAgentRail && $agentPlacement === 'right'}
        <AgentSideRail side="right" />
      {/if}
    </div>
  {/if}
  {#if shell.showReviewDock}
    <!-- VEIL source + Source preview (generated) live in the bottom dock -->
    <ReviewDock />
  {:else if shell.showCodePreview}
    <!-- Fallback floating preview when review dock is off (e.g. flow-only shell) -->
    <CodePreview />
  {/if}

  {#if attachOpen && shell.attachPickerOnConnect}
    <div class="attach-modal" role="dialog" aria-modal="true" aria-label="Attach node">
      <div class="attach-modal__backdrop" onclick={closeAttachPicker}></div>
      <div class="attach-modal__panel">
        <header class="attach-modal__head">
          <h2 class="attach-modal__title">Attach next step</h2>
          <p class="attach-modal__sub">
            Choose a construct from <code>{flowLayer || 'layer'}</code>
            {#if attachSourceId}
              · from node #{attachSourceId}
            {/if}
          </p>
          <button type="button" class="attach-modal__close" onclick={closeAttachPicker}>✕</button>
        </header>
        <div class="attach-modal__grid">
          {#each ($paletteConfig || []).filter((c) => (c.entry_type || 'construct') === 'construct') as item}
            <button
              type="button"
              class="attach-modal__tile"
              style="--tile-color: {item.color || 'var(--veil-text-dim)'}"
              title={item.description || item.label}
              onclick={() => void attachPaletteItem(item as PaletteEntry)}
            >
              <span class="attach-modal__icon">{item.icon || '◇'}</span>
              <span class="attach-modal__label">{item.label || item.keyword || item.name}</span>
              {#if item.description}
                <span class="attach-modal__desc">{item.description}</span>
              {/if}
            </button>
          {/each}
        </div>
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
    background: var(--veil-bg);
  }

  /* Flow composer: fill viewport — palette | canvas | agent only */
  .viewer-container--flow {
    background: var(--veil-bg, #0f0f14);
  }
  .main-layout--flow {
    flex: 1;
    min-height: 0;
    height: 100%;
  }
  .viewer-container--flow .graph-wrapper {
    min-height: 0;
  }

  .attach-modal {
    position: fixed;
    inset: 0;
    z-index: 12000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1rem;
  }
  .attach-modal__backdrop {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.65);
    backdrop-filter: blur(2px);
  }
  .attach-modal__panel {
    position: relative;
    z-index: 1;
    width: min(32rem, 94vw);
    max-height: min(70vh, 36rem);
    overflow: auto;
    border-radius: 0.85rem;
    border: 1px solid var(--veil-border, #2a2a38);
    background: var(--veil-surface, #14141c);
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5);
  }
  .attach-modal__head {
    position: sticky;
    top: 0;
    padding: 0.85rem 1rem 0.65rem;
    border-bottom: 1px solid var(--veil-border, #2a2a38);
    background: var(--veil-surface, #14141c);
  }
  .attach-modal__title {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 700;
  }
  .attach-modal__sub {
    margin: 0.25rem 0 0;
    font-size: 0.75rem;
    color: var(--veil-text-dim, #9ca3af);
  }
  .attach-modal__close {
    position: absolute;
    top: 0.65rem;
    right: 0.65rem;
    border: 0;
    background: transparent;
    color: var(--veil-text-dim, #9ca3af);
    cursor: pointer;
    font-size: 1rem;
  }
  .attach-modal__grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(8.5rem, 1fr));
    gap: 0.55rem;
    padding: 0.85rem;
  }
  .attach-modal__tile {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 0.2rem;
    padding: 0.65rem 0.7rem;
    border-radius: 0.55rem;
    border: 1px solid color-mix(in srgb, var(--tile-color) 40%, var(--veil-border, #2a2a38));
    background: color-mix(in srgb, var(--tile-color) 12%, transparent);
    color: inherit;
    cursor: pointer;
    text-align: left;
  }
  .attach-modal__tile:hover {
    border-color: var(--tile-color);
  }
  .attach-modal__icon { font-size: 1.1rem; }
  .attach-modal__label { font-size: 0.82rem; font-weight: 650; }
  .attach-modal__desc {
    font-size: 0.68rem;
    color: var(--veil-text-dim, #9ca3af);
    line-height: 1.3;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .agent-activity-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 12px;
    background: rgba(96, 165, 250, 0.08);
    border-bottom: 1px solid rgba(96, 165, 250, 0.2);
    font-size: 11px;
    color: #60a5fa;
    animation: agent-bar-in 0.2s ease-out;
  }

  .agent-activity-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #60a5fa;
    animation: agent-pulse 1s ease-in-out infinite;
  }

  .agent-activity-text {
    font-weight: 500;
  }

  @keyframes agent-bar-in {
    from { opacity: 0; transform: translateY(-4px); }
    to { opacity: 1; transform: translateY(0); }
  }

  @keyframes agent-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
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

  .add-file-btn {
    background: var(--veil-input-bg);
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    padding: 2px 10px;
    font-size: 16px;
    font-weight: 700;
    line-height: 1.2;
    color: var(--veil-accent);
    cursor: pointer;
    flex-shrink: 0;
  }
  .add-file-btn:hover {
    background: var(--veil-accent-subtle);
    border-color: var(--veil-accent);
  }

  .new-file-form {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .kind-select { max-width: 140px; font-weight: 500; }
  .new-file-input {
    width: 120px;
    padding: 4px 8px;
    border-radius: 6px;
    border: 1px solid var(--veil-border);
    background: var(--veil-input-bg);
    color: var(--veil-text);
    font-size: 12px;
  }
  .new-file-input:focus {
    outline: none;
    border-color: var(--veil-accent);
  }
  .new-file-go {
    padding: 4px 10px;
    font-size: 12px;
  }

  .project-badge {
    font-size: 12px;
    font-weight: 700;
    color: var(--veil-text);
    background: var(--veil-accent-subtle);
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    padding: 3px 10px;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .project-switcher { max-width: 160px; font-weight: 600; }

  .hub-picker { text-align: left; max-width: 480px; }
  .hub-list { list-style: none; padding: 0; margin: 16px 0; display: flex; flex-direction: column; gap: 10px; }
  .hub-list li { display: flex; align-items: center; gap: 12px; }
  .hub-meta { font-size: 12px; color: var(--veil-text-dim); }
  .hub-create { display: flex; gap: 8px; margin: 16px 0; }
  .hub-input {
    flex: 1;
    padding: 8px 12px;
    border-radius: 6px;
    border: 1px solid var(--veil-border);
    background: var(--veil-input-bg);
    color: var(--veil-text);
  }

  .kind-badge {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: #a5b4fc;
    background: rgba(99, 102, 241, 0.15);
    border: 1px solid rgba(99, 102, 241, 0.35);
    border-radius: 4px;
    padding: 2px 8px;
    margin-right: 8px;
  }

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
    display: flex;
    flex-direction: column;
  }

  .flow-nav-bar {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    flex-shrink: 0;
    padding: 0.4rem 0.75rem;
    border-bottom: 1px solid var(--veil-border, #2e2e2e);
    background: var(--veil-surface-alt, #1a1a1a);
    z-index: 40;
    position: relative;
  }

  .flow-back-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    border: 1px solid var(--veil-border, #2e2e2e);
    background: transparent;
    color: var(--veil-text, #e5e5e5);
    font: inherit;
    font-size: 0.78rem;
    font-weight: 600;
    padding: 0.28rem 0.55rem;
    border-radius: 6px;
    cursor: pointer;
    transition:
      background 140ms ease,
      border-color 140ms ease;
  }
  .flow-back-btn:hover {
    border-color: var(--veil-text-dim, #a3a3a3);
    background: color-mix(in srgb, var(--veil-text-dim, #a3a3a3) 12%, transparent);
  }

  .flow-nav-sep {
    color: var(--veil-text-faint, #737373);
  }

  .flow-nav-here {
    font-weight: 650;
    font-size: 0.82rem;
    font-family: var(--font-mono, monospace);
    color: var(--veil-text, #e5e5e5);
  }

  .flow-nav-kind {
    font-size: 0.65rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--veil-text-faint, #737373);
    padding: 0.1rem 0.35rem;
    border-radius: 4px;
    border: 1px solid var(--veil-border, #2e2e2e);
  }

  .flow-nav-hint {
    margin-left: auto;
    font-size: 0.68rem;
    color: var(--veil-text-faint, #737373);
  }

  /* Native layout renderers (tree/flat) */
  .native-layout-container {
    display: flex;
    width: 100%;
    height: 100%;
  }

  .native-layout-container.resizing {
    cursor: col-resize;
    user-select: none;
  }

  .tree-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.45rem 0.65rem;
    border-bottom: 1px solid var(--veil-border);
    background: var(--veil-surface-alt, rgba(26, 26, 26, 0.95));
    flex-shrink: 0;
  }

  .tree-toolbar-title {
    font-size: 0.68rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--veil-text-dim, #a3a3a3);
  }

  .native-layout-sidebar {
    overflow: hidden;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    border-right: none;
  }

  .outline-resize-handle {
    width: 6px;
    flex-shrink: 0;
    cursor: col-resize;
    touch-action: none;
    background: var(--veil-border);
    position: relative;
    z-index: 5;
    transition: background 0.12s ease;
  }

  .outline-resize-handle:hover,
  .native-layout-container.resizing .outline-resize-handle {
    background: var(--veil-accent, #a3a3a3);
  }

  /* Wider hit target without eating layout */
  .outline-resize-handle::after {
    content: '';
    position: absolute;
    inset: 0 -5px;
  }

  :global(body.outline-resizing) {
    cursor: col-resize !important;
    user-select: none !important;
  }

  :global(body.outline-resizing iframe) {
    pointer-events: none !important;
  }

  .native-layout-sidebar :global(.tree-layout),
  .native-layout-sidebar :global(.flat-layout) {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .native-layout-detail {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    background: var(--veil-surface);
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

  .critical-count {
    font-size: 10px;
    opacity: 0.75;
    margin-left: 2px;
  }

  /* Multi-lens picker (LAY-009) */
  .lens-picker-wrapper {
    position: relative;
  }

  .lens-picker-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border: 1px solid var(--veil-border);
    border-radius: 4px;
    background: transparent;
    color: var(--veil-text-dim);
    font-size: 11px;
    cursor: pointer;
    transition: all 0.15s;
  }

  .lens-picker-btn:hover {
    background: var(--veil-accent-hover);
    color: var(--veil-text-secondary);
  }

  .lens-picker-btn.active {
    border-color: var(--veil-accent);
    color: var(--veil-text);
    background: var(--veil-accent-subtle);
  }

  .lens-active-count {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 8px;
    background: var(--veil-accent);
    color: var(--veil-bg);
    font-weight: 600;
  }

  .lens-picker-popover {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    min-width: 160px;
    padding: 6px;
    background: var(--veil-surface);
    border: 1px solid var(--veil-border);
    border-radius: 6px;
    box-shadow: 0 4px 12px var(--veil-shadow);
    z-index: 100;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .lens-picker-item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    border-radius: 4px;
    font-size: 12px;
    cursor: pointer;
    transition: background 0.1s;
  }

  .lens-picker-item:hover {
    background: var(--veil-accent-hover);
  }

  .lens-picker-item input[type="checkbox"] {
    width: 13px;
    height: 13px;
    accent-color: var(--veil-accent);
  }

  .lens-name {
    flex: 1;
    color: var(--veil-text);
    text-transform: capitalize;
  }

  .lens-count {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 8px;
    background: var(--veil-accent-subtle);
    color: var(--veil-text-faint);
  }

  .lens-clear-btn {
    margin-top: 4px;
    padding: 4px 8px;
    border: none;
    border-top: 1px solid var(--veil-border);
    background: transparent;
    color: var(--veil-text-dim);
    font-size: 11px;
    cursor: pointer;
    text-align: center;
  }

  .lens-clear-btn:hover {
    color: var(--veil-text);
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

  :global(.svelte-flow) {
    background: var(--veil-bg) !important;
    flex: 1;
    min-height: 0;
  }
  :global(.svelte-flow__background) { opacity: 0.4; }
  :global(.svelte-flow__minimap) { background: var(--veil-surface-alt) !important; border: 1px solid var(--veil-border) !important; border-radius: 10px !important; }
  :global(.svelte-flow__controls) { background: var(--veil-surface-alt) !important; border: 1px solid var(--veil-border) !important; border-radius: 10px !important; }
  :global(.svelte-flow__controls button) { background: transparent !important; border-color: var(--veil-border) !important; color: var(--veil-text) !important; }
  :global(.svelte-flow__controls button:hover) { background: var(--veil-accent-hover) !important; }
  :global(.svelte-flow__edge-path) { stroke-width: 2px; filter: drop-shadow(0 0 3px rgba(100, 100, 100, 0.2)); }
  :global(.svelte-flow__edge.animated .svelte-flow__edge-path) { stroke: var(--veil-text-dim) !important; stroke-width: 2.5px; filter: drop-shadow(0 0 6px rgba(100, 100, 100, 0.3)); }
  :global(.svelte-flow__handle) { width: 8px !important; height: 8px !important; background: var(--node-color, var(--veil-text-faint)) !important; border: 2px solid var(--veil-surface) !important; opacity: 0; transition: opacity 0.2s; }
  :global(.svelte-flow__node:hover .svelte-flow__handle) { opacity: 1; }

  .fn-signature-bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    background: var(--veil-surface);
    border-bottom: 1px solid var(--veil-border);
    font-family: 'JetBrains Mono', monospace;
    font-size: 13px;
    z-index: 5;
  }
  .fn-kw {
    color: var(--veil-accent);
    font-weight: 700;
  }
  .fn-name {
    color: var(--veil-text);
    font-weight: 600;
  }
  .fn-params {
    color: var(--veil-text-dim);
  }
  .fn-arrow {
    color: var(--veil-text-faint);
  }
  .fn-return {
    color: var(--veil-accent);
  }
</style>
