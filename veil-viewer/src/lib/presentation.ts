/**
 * Layer-driven presentation projection (LAY-003).
 *
 * Consumes PresentationModel from GET /api/presentation. Never matches on
 * domain keywords (agg, ctx, …) — only construct names / subkinds from IR +
 * presentation IR. See docs/PRESENTATION.md.
 */

import type { IrGraph, IrNode } from './types';

export interface NestRule {
  child: string;
  parent: string;
  when?: string;
}

export interface ViewSpec {
  id: string;
  label: string;
  layout: string;
  is_default?: boolean;
  members?: string;
  roots?: string[];
  nest_rules?: NestRule[];
  orphan_policy?: string;
  tabs?: string[];
  left?: string[];
  right?: string[];
  edge?: string | null;
}

export interface HostPresentation {
  default_view?: string | null;
  views: ViewSpec[];
}

export interface ConstructRoleDto {
  role?: string | null;
  lenses?: string[];
  default_view?: string | null;
}

export interface PresentationModel {
  version: number;
  hosts: Record<string, HostPresentation>;
  constructs: Record<string, ConstructRoleDto>;
}

/** MVP layouts (LAY-006). Unknown ids fall back to `flat` at runtime. */
export const MVP_LAYOUTS = ['flat', 'tabs', 'tree', 'flow'] as const;
export type MvpLayout = (typeof MVP_LAYOUTS)[number];

export interface ProjectResult {
  /** Nodes to place on the canvas (top-level of this projection). */
  nodes: IrNode[];
  /** For layout `tabs`: ordered tab keys. */
  tabs: string[];
  /** Active tab contents when layout is tabs (group IR node or null for virtual). */
  tabGroupNodes: Map<string, IrNode | null>;
  /** Resolved layout after unknown→flat fallback. */
  layout: string;
  /** True if view.layout was unknown / deferred and remapped. */
  layoutFallback: boolean;
  /** For `flow`: ELK direction. */
  flowDirection: 'LR' | 'TB' | null;
  /** Nest attachments child→parent (LAY-007). */
  nestEdges: { child: number; parent: number }[];
  /** Orphans (not root, not nested). */
  orphanIds: number[];
  /** Synthetic bucket label when orphan_policy is bucket[:Name]. */
  orphanBucketLabel: string | null;
  view: ViewSpec | null;
}

/** Resolve layout: known MVP pass through; bipartite/unknown → flat + fallback. */
export function resolveLayout(layout: string | undefined | null): {
  layout: string;
  fallback: boolean;
} {
  const l = (layout ?? '').trim();
  if (!l) return { layout: 'flat', fallback: true };
  if ((MVP_LAYOUTS as readonly string[]).includes(l)) {
    return { layout: l, fallback: false };
  }
  // bipartite deferred; anything else unknown
  return { layout: 'flat', fallback: true };
}

function constructName(n: IrNode): string {
  return n.metadata.subkind ?? n.kind;
}

function sortBySpan(a: IrNode, b: IrNode): number {
  return a.span.start - b.span.start || a.name.localeCompare(b.name);
}

/** Direct IR children of parent. */
export function irChildren(graph: IrGraph, parentId: number): IrNode[] {
  return graph.nodes.filter((n) => n.metadata.parent === parentId).sort(sortBySpan);
}

/** All descendants of parent (BFS). */
export function irDescendants(graph: IrGraph, parentId: number): IrNode[] {
  const out: IrNode[] = [];
  const queue = [parentId];
  const seen = new Set<number>([parentId]);
  while (queue.length) {
    const id = queue.shift()!;
    for (const c of irChildren(graph, id)) {
      if (seen.has(c.id)) continue;
      seen.add(c.id);
      out.push(c);
      queue.push(c.id);
    }
  }
  return out.sort(sortBySpan);
}

/**
 * Flatten core Group nodes one or more levels: organizational buckets are
 * transparent for candidate collection (paradigm-agnostic — Group is a core shape).
 */
export function flattenGroups(graph: IrGraph, nodes: IrNode[]): IrNode[] {
  const out: IrNode[] = [];
  for (const n of nodes) {
    if (n.kind === 'Group') {
      out.push(...flattenGroups(graph, irChildren(graph, n.id)));
    } else {
      out.push(n);
    }
  }
  return out.sort(sortBySpan);
}

function collectCandidates(
  graph: IrGraph,
  hostId: number,
  members: string,
  view: ViewSpec
): IrNode[] {
  const layout = resolveLayout(view.layout).layout;
  const mode = members || defaultMembers(layout);
  if (mode === 'all_descendants') {
    return irDescendants(graph, hostId);
  }
  if (mode === 'by_construct') {
    const names = new Set<string>([
      ...(view.roots ?? []),
      ...(view.nest_rules ?? []).flatMap((r) => [r.child, r.parent]),
    ]);
    return irDescendants(graph, hostId).filter((n) => names.has(constructName(n)));
  }
  // by_host_children (default) and by_source_group start from direct children.
  // For non-tab layouts, flatten Group containers so roots can be found inside.
  const direct = irChildren(graph, hostId);
  if (layout === 'tabs' || mode === 'by_source_group') {
    return direct;
  }
  const top = flattenGroups(graph, direct);
  // Tree: include full subtrees under flattened tops so nest rules see
  // children of roots (e.g. Event under Aggregate under group domain).
  if (layout === 'tree') {
    const seen = new Set<number>();
    const out: IrNode[] = [];
    for (const n of top) {
      if (!seen.has(n.id)) {
        seen.add(n.id);
        out.push(n);
      }
      for (const d of irDescendants(graph, n.id)) {
        if (!seen.has(d.id)) {
          seen.add(d.id);
          out.push(d);
        }
      }
    }
    return out.sort(sortBySpan);
  }
  return top;
}

function defaultMembers(layout: string): string {
  if (layout === 'tabs') return 'by_source_group';
  return 'by_host_children';
}

/** Walk ancestors of node; return first with construct name === want, or null. */
function ancestorWithName(
  graph: IrGraph,
  node: IrNode,
  want: string
): IrNode | null {
  let pid = node.metadata.parent;
  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  const seen = new Set<number>();
  while (pid != null && !seen.has(pid)) {
    seen.add(pid);
    const p = byId.get(pid);
    if (!p) break;
    if (constructName(p) === want) return p;
    pid = p.metadata.parent;
  }
  return null;
}

function nearestGroupName(graph: IrGraph, node: IrNode): string | null {
  let pid = node.metadata.parent;
  const byId = new Map(graph.nodes.map((n) => [n.id, n]));
  const seen = new Set<number>();
  while (pid != null && !seen.has(pid)) {
    seen.add(pid);
    const p = byId.get(pid);
    if (!p) break;
    if (p.kind === 'Group') return p.name;
    pid = p.metadata.parent;
  }
  return null;
}

function implementsEdge(graph: IrGraph, a: number, b: number): boolean {
  return graph.edges.some(
    (e) =>
      e.kind === 'Implements' &&
      ((e.from === a && e.to === b) || (e.from === b && e.to === a))
  );
}

/** Candidate parents ordered for ambiguity (AST prefer, then id). */
function candidateParents(
  graph: IrGraph,
  child: IrNode,
  parentType: string,
  when: string,
  candidates: IrNode[],
  candidateIds: Set<number>
): IrNode[] {
  const parents = candidates.filter((p) => {
    if (constructName(p) !== parentType || p.id === child.id) return false;
    switch (when) {
      case 'declared_in_parent':
      case 'in_parent_type':
        return ancestorWithName(graph, child, parentType)?.id === p.id;
      case 'same_source_group': {
        const cg = nearestGroupName(graph, child);
        const pg = nearestGroupName(graph, p);
        return cg != null && cg === pg && candidateIds.has(p.id);
      }
      case 'always':
        return true;
      case 'implements':
        return implementsEdge(graph, child.id, p.id);
      default:
        return ancestorWithName(graph, child, parentType)?.id === p.id;
    }
  });
  const astPref = ancestorWithName(graph, child, parentType)?.id;
  parents.sort((a, b) => {
    const pa = a.id === astPref ? 0 : 1;
    const pb = b.id === astPref ? 0 : 1;
    return pa - pb || a.id - b.id;
  });
  return parents;
}

function wouldCycle(
  nest: Map<number, number>,
  child: number,
  parent: number
): boolean {
  if (child === parent) return true;
  let walk: number | undefined = parent;
  const seen = new Set<number>();
  while (walk != null) {
    if (walk === child) return true;
    if (seen.has(walk)) return true;
    seen.add(walk);
    walk = nest.get(walk);
  }
  return false;
}

export function parseOrphanPolicy(raw: string | undefined | null): {
  mode: string;
  bucketLabel: string | null;
} {
  const s = (raw ?? '').trim();
  if (!s) return { mode: 'list', bucketLabel: null };
  if (s === 'list' || s === 'hide') return { mode: s, bucketLabel: null };
  if (s === 'bucket') return { mode: 'bucket', bucketLabel: 'Other' };
  if (s.startsWith('bucket:')) {
    const name = s.slice(7).trim();
    return { mode: 'bucket', bucketLabel: name || 'Other' };
  }
  if (s.startsWith('bucket ')) {
    const name = s.slice(7).trim();
    return { mode: 'bucket', bucketLabel: name || 'Other' };
  }
  return { mode: 'list', bucketLabel: null };
}

/**
 * Project children of `hostId` under the given view.
 * When view is null, returns empty projection (caller uses legacy fallback).
 */
export function projectView(
  graph: IrGraph,
  hostId: number,
  view: ViewSpec | null,
  options?: { hideLayerProvided?: boolean }
): ProjectResult {
  const empty = {
    nodes: [] as IrNode[],
    tabs: [] as string[],
    tabGroupNodes: new Map<string, IrNode | null>(),
    layout: 'flat',
    layoutFallback: false,
    flowDirection: null as 'LR' | 'TB' | null,
    nestEdges: [] as { child: number; parent: number }[],
    orphanIds: [] as number[],
    orphanBucketLabel: null as string | null,
    view: null as ViewSpec | null,
  };

  if (!view) {
    return empty;
  }

  const { layout, fallback: layoutFallback } = resolveLayout(view.layout);
  const effectiveView: ViewSpec = { ...view, layout };

  let candidates = collectCandidates(graph, hostId, view.members ?? '', effectiveView);
  if (options?.hideLayerProvided !== false) {
    candidates = candidates.filter(
      (c) => !c.metadata.annotations.includes('layer-provided')
    );
  }
  const candidateIds = new Set(candidates.map((c) => c.id));

  if (layout === 'tabs') {
    return projectTabs(graph, effectiveView, candidates, layoutFallback);
  }

  if (layout === 'tree') {
    return projectTree(graph, effectiveView, candidates, candidateIds, layoutFallback);
  }

  if (layout === 'flow') {
    return {
      ...empty,
      nodes: [...candidates].sort(sortBySpan),
      layout: 'flow',
      layoutFallback,
      flowDirection: 'LR',
      view: effectiveView,
    };
  }

  return {
    ...empty,
    nodes: [...candidates].sort(sortBySpan),
    layout: 'flat',
    layoutFallback,
    view: effectiveView,
  };
}

function projectTabs(
  graph: IrGraph,
  view: ViewSpec,
  candidates: IrNode[],
  layoutFallback: boolean
): ProjectResult {
  const groupNodes = candidates.filter((c) => c.kind === 'Group');
  const tabKeys =
    view.tabs && view.tabs.length > 0
      ? [...view.tabs]
      : [...new Set(groupNodes.map((g) => g.name))];

  for (const g of groupNodes) {
    if (!tabKeys.includes(g.name)) tabKeys.push(g.name);
  }

  const tabGroupNodes = new Map<string, IrNode | null>();
  for (const key of tabKeys) {
    tabGroupNodes.set(key, groupNodes.find((g) => g.name === key) ?? null);
  }

  return {
    nodes: [],
    tabs: tabKeys,
    tabGroupNodes,
    layout: 'tabs',
    layoutFallback,
    flowDirection: null,
    nestEdges: [],
    orphanIds: [],
    orphanBucketLabel: null,
    view,
  };
}

function projectTree(
  graph: IrGraph,
  view: ViewSpec,
  candidates: IrNode[],
  candidateIds: Set<number>,
  layoutFallback: boolean
): ProjectResult {
  const rootNames = new Set(view.roots ?? []);
  const rules = view.nest_rules ?? [];
  const nest = new Map<number, number>();

  for (const rule of rules) {
    const when = rule.when || 'declared_in_parent';
    for (const c of candidates) {
      if (constructName(c) !== rule.child) continue;
      if (nest.has(c.id)) continue;
      const parents = candidateParents(
        graph,
        c,
        rule.parent,
        when,
        candidates,
        candidateIds
      );
      const p = parents[0];
      if (p && !wouldCycle(nest, c.id, p.id)) {
        nest.set(c.id, p.id);
      }
    }
  }

  const nestedIds = new Set(nest.keys());
  const isRoot = (n: IrNode) =>
    rootNames.size === 0 ? !nestedIds.has(n.id) : rootNames.has(constructName(n));

  const roots = candidates.filter((n) => isRoot(n) && !nestedIds.has(n.id));
  const orphans = candidates.filter((n) => !isRoot(n) && !nestedIds.has(n.id));
  const { mode, bucketLabel } = parseOrphanPolicy(view.orphan_policy);

  let nodes: IrNode[] = [...roots];
  if (mode === 'list') {
    nodes = [...roots, ...orphans];
  } else if (mode === 'hide' || mode === 'bucket') {
    nodes = [...roots];
  }
  nodes.sort(sortBySpan);

  const nestEdges = [...nest.entries()]
    .map(([child, parent]) => ({ child, parent }))
    .sort((a, b) => a.child - b.child || a.parent - b.parent);

  return {
    nodes,
    tabs: [],
    tabGroupNodes: new Map(),
    layout: 'tree',
    layoutFallback,
    flowDirection: null,
    nestEdges,
    orphanIds: orphans.map((o) => o.id),
    orphanBucketLabel: mode === 'bucket' ? bucketLabel : null,
    view,
  };
}

/** Host views for a construct name (subkind), or empty. */
export function viewsForHost(
  model: PresentationModel | null,
  hostConstructName: string | null | undefined
): ViewSpec[] {
  if (!model || !hostConstructName) return [];
  return model.hosts[hostConstructName]?.views ?? [];
}

export function pickDefaultView(
  host: HostPresentation | undefined,
  views: ViewSpec[]
): string | null {
  if (!views.length) return null;
  if (host?.default_view && views.some((v) => v.id === host.default_view)) {
    return host.default_view;
  }
  const marked = views.find((v) => v.is_default);
  if (marked) return marked.id;
  return views[0].id;
}

/**
 * Self-check used as a lightweight genericity proof (no DDD keywords).
 * Returns null on success, error message on failure.
 */
export function selfCheckProjection(): string | null {
  const graph: IrGraph = {
    next_id: 10,
    edges: [],
    nodes: [
      {
        id: 1,
        kind: 'Module',
        name: 'H',
        span: { start: 0, end: 1 },
        metadata: { parent: null, annotations: [], properties: [], subkind: 'Host' },
      },
      {
        id: 2,
        kind: 'Group',
        name: 'bucket',
        span: { start: 2, end: 3 },
        metadata: { parent: 1, annotations: [], properties: [], subkind: 'Group' },
      },
      {
        id: 3,
        kind: 'TypeDef',
        name: 'RootA',
        span: { start: 4, end: 5 },
        metadata: { parent: 2, annotations: [], properties: [], subkind: 'RootType' },
      },
      {
        id: 4,
        kind: 'TypeDef',
        name: 'ChildB',
        span: { start: 6, end: 7 },
        metadata: { parent: 3, annotations: [], properties: [], subkind: 'ChildType' },
      },
      {
        id: 5,
        kind: 'TypeDef',
        name: 'OrphanC',
        span: { start: 8, end: 9 },
        metadata: { parent: 2, annotations: [], properties: [], subkind: 'OtherType' },
      },
    ],
  };
  const view: ViewSpec = {
    id: 'model',
    label: 'Model',
    layout: 'tree',
    members: 'by_host_children',
    roots: ['RootType'],
    nest_rules: [
      { child: 'ChildType', parent: 'RootType', when: 'declared_in_parent' },
    ],
    orphan_policy: 'list',
  };
  const result = projectView(graph, 1, view, { hideLayerProvided: true });
  const names = result.nodes.map((n) => n.name).sort();
  if (!names.includes('RootA')) return 'missing root RootA';
  if (names.includes('ChildB')) return 'ChildB should be nested under RootA, not top-level';
  if (!names.includes('OrphanC')) return 'orphan OrphanC should list';
  if (result.layout !== 'tree') return 'layout should be tree';

  // LAY-006: unknown layout falls back to flat
  const bad = projectView(
    graph,
    1,
    { id: 'x', label: 'X', layout: 'spiral' },
    { hideLayerProvided: true }
  );
  if (bad.layout !== 'flat' || !bad.layoutFallback) {
    return 'unknown layout should fall back to flat';
  }

  // flow
  const flow = projectView(
    graph,
    1,
    { id: 'f', label: 'F', layout: 'flow', members: 'by_host_children' },
    { hideLayerProvided: true }
  );
  if (flow.layout !== 'flow' || flow.flowDirection !== 'LR') {
    return 'flow layout should set LR direction';
  }

  return null;
}
