/**
 * Resolve parent_span for create_construct from presentation + selection (LAY-008).
 * Zero domain knowledge — uses construct names / roles / dg from palette + presentation IR.
 */

import type { IrGraph, IrNode } from './types';
import type { PresentationModel, ViewSpec } from './presentation';

export interface PlaceableItem {
  /** Layer construct name (e.g. Aggregate). */
  name?: string;
  /** Surface keyword for create_construct (e.g. agg). */
  keyword?: string;
  label?: string;
  /** Default group from palette `dg`. */
  dg?: string;
  /** Preferred source group from palette `group`. */
  group?: string;
}

export interface PlacementContext {
  graph: IrGraph;
  /** Current drill parent IR id. */
  hostId: number | null;
  /** Selected node IR id (if any). */
  selectedId: number | null;
  /** Active group tab name (Layers view). */
  activeGroup: string | null;
  /** Active presentation view id. */
  activeViewId: string | null;
  presentation: PresentationModel | null;
}

/**
 * Choose parent_span for a new construct. Returns null if host has no span
 * (unsaved / missing).
 */
export function resolveCreateParentSpan(
  item: PlaceableItem,
  ctx: PlacementContext
): { parentSpan: number; reason: string } | null {
  const { graph, hostId, selectedId, activeGroup, activeViewId, presentation } = ctx;
  if (hostId == null) return null;

  const host = graph.nodes.find((n) => n.id === hostId);
  if (!host) return null;

  const selected =
    selectedId != null ? graph.nodes.find((n) => n.id === selectedId) : undefined;
  const hostSubkind = host.metadata.subkind ?? null;
  const view: ViewSpec | undefined =
    presentation && hostSubkind && activeViewId
      ? presentation.hosts[hostSubkind]?.views.find((v) => v.id === activeViewId)
      : undefined;

  // 1) Selection is a container role → nest under it
  if (selected && selected.id !== hostId) {
    const selName = selected.metadata.subkind ?? '';
    const role = presentation?.constructs[selName]?.role;
    const nestable = presentation?.constructs[item.name ?? '']?.nestable ?? [];
    const nestUnder = nestable.some(
      (n) =>
        n.under === selName &&
        (!activeViewId || n.view_id === activeViewId || n.under === 'root')
    );
    // Also: nest rule child under selected construct type
    const ruleOk = view?.nest_rules?.some(
      (r) => r.child === item.name && r.parent === selName
    );
    if (role === 'container' || nestUnder || ruleOk) {
      return {
        parentSpan: selected.span.start,
        reason: `selected container ${selected.name}`,
      };
    }
  }

  // 2) Active group tab (Layers / Folders views)
  const groupName = activeGroup || item.dg || item.group || null;
  if (groupName) {
    const groupNode = graph.nodes.find(
      (n) =>
        n.kind === 'Group' &&
        n.name === groupName &&
        n.metadata.parent === hostId
    );
    if (groupNode) {
      return {
        parentSpan: groupNode.span.start,
        reason: `group ${groupName}`,
      };
    }
  }

  // 3) Fall back to host (context / app / module)
  return {
    parentSpan: host.span.start,
    reason: `host ${host.name}`,
  };
}

export function uniqueConstructName(
  graph: IrGraph,
  base: string,
  parentId: number | null
): string {
  const siblings = graph.nodes.filter((n) => n.metadata.parent === parentId);
  if (!siblings.some((s) => s.name === base)) return base;
  let i = 2;
  while (siblings.some((s) => s.name === `${base}${i}`)) i++;
  return `${base}${i}`;
}

/** Find IR parent id for a span start (best-effort). */
export function nodeIdForSpan(graph: IrGraph, spanStart: number): number | null {
  const n = graph.nodes.find((x) => x.span.start === spanStart);
  return n?.id ?? null;
}
