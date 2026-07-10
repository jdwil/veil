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

export interface ProjectResult {
  /** Nodes to place on the canvas (top-level of this projection). */
  nodes: IrNode[];
  /** For layout `tabs`: ordered tab keys. */
  tabs: string[];
  /** Active tab contents when layout is tabs (group IR node or null for virtual). */
  tabGroupNodes: Map<string, IrNode | null>;
  layout: string;
  view: ViewSpec | null;
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
  const mode = members || defaultMembers(view.layout);
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
  if (view.layout === 'tabs' || mode === 'by_source_group') {
    return direct;
  }
  return flattenGroups(graph, direct);
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

function isNestedUnderRule(
  graph: IrGraph,
  child: IrNode,
  rule: NestRule,
  candidateIds: Set<number>
): boolean {
  if (constructName(child) !== rule.child) return false;
  const when = rule.when || 'declared_in_parent';
  if (when === 'always') {
    // Attach if any candidate has parent type
    return [...candidateIds].some((id) => {
      const n = graph.nodes.find((x) => x.id === id);
      return n && constructName(n) === rule.parent;
    });
  }
  // declared_in_parent / in_parent_type / same_source_group (approx)
  const anc = ancestorWithName(graph, child, rule.parent);
  return anc != null && candidateIds.has(anc.id);
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
  if (!view) {
    return {
      nodes: [],
      tabs: [],
      tabGroupNodes: new Map(),
      layout: 'flat',
      view: null,
    };
  }

  let candidates = collectCandidates(graph, hostId, view.members ?? '', view);
  if (options?.hideLayerProvided !== false) {
    candidates = candidates.filter(
      (c) => !c.metadata.annotations.includes('layer-provided')
    );
  }
  const candidateIds = new Set(candidates.map((c) => c.id));

  if (view.layout === 'tabs') {
    return projectTabs(graph, hostId, view, candidates, options);
  }

  if (view.layout === 'tree') {
    return projectTree(graph, view, candidates, candidateIds);
  }

  // flat | flow | bipartite (MVP: flat sibling list)
  return {
    nodes: [...candidates].sort(sortBySpan),
    tabs: [],
    tabGroupNodes: new Map(),
    layout: view.layout || 'flat',
    view,
  };
}

function projectTabs(
  graph: IrGraph,
  hostId: number,
  view: ViewSpec,
  candidates: IrNode[],
  options?: { hideLayerProvided?: boolean }
): ProjectResult {
  const groupNodes = candidates.filter((c) => c.kind === 'Group');
  const tabKeys =
    view.tabs && view.tabs.length > 0
      ? [...view.tabs]
      : [...new Set(groupNodes.map((g) => g.name))];

  // Ensure existing groups appear even if not in view.tabs
  for (const g of groupNodes) {
    if (!tabKeys.includes(g.name)) tabKeys.push(g.name);
  }

  const tabGroupNodes = new Map<string, IrNode | null>();
  for (const key of tabKeys) {
    tabGroupNodes.set(key, groupNodes.find((g) => g.name === key) ?? null);
  }

  return {
    nodes: [], // tab contents resolved by caller for active tab
    tabs: tabKeys,
    tabGroupNodes,
    layout: 'tabs',
    view,
  };
}

function projectTree(
  graph: IrGraph,
  view: ViewSpec,
  candidates: IrNode[],
  candidateIds: Set<number>
): ProjectResult {
  const rootNames = new Set(view.roots ?? []);
  const rules = view.nest_rules ?? [];

  const nestedIds = new Set<number>();
  for (const rule of rules) {
    for (const c of candidates) {
      if (isNestedUnderRule(graph, c, rule, candidateIds)) {
        nestedIds.add(c.id);
      }
    }
  }

  const isRoot = (n: IrNode) =>
    rootNames.size === 0 ? !nestedIds.has(n.id) : rootNames.has(constructName(n));

  const roots = candidates.filter((n) => isRoot(n) && !nestedIds.has(n.id));
  const orphans = candidates.filter((n) => !isRoot(n) && !nestedIds.has(n.id));

  const policy = view.orphan_policy || 'list';
  let nodes: IrNode[] = [...roots];
  if (policy === 'list') {
    nodes = [...roots, ...orphans];
  } else if (policy === 'hide') {
    nodes = [...roots];
  } else if (policy === 'bucket') {
    // Synthetic bucket not an IrNode — orphans still listed as roots siblings for MVP
    nodes = [...roots, ...orphans];
  }

  nodes.sort(sortBySpan);
  return {
    nodes,
    tabs: [],
    tabGroupNodes: new Map(),
    layout: 'tree',
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
  // RootA is root; ChildB nested (hidden); OrphanC listed
  if (!names.includes('RootA')) return 'missing root RootA';
  if (names.includes('ChildB')) return 'ChildB should be nested under RootA, not top-level';
  if (!names.includes('OrphanC')) return 'orphan OrphanC should list';
  if (result.layout !== 'tree') return 'layout should be tree';
  return null;
}
