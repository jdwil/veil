/**
 * Layer-declared review lenses (LAY-009 / UX-022).
 * Criticality comes from presentation IR + diagnostics — not keyword lists.
 */

import type { IrGraph, IrNode } from './types';
import type { PresentationModel } from './presentation';
import type { Diagnostic } from './store';

const ESCAPE_CODES = new Set([
  'escape_raw',
  'escape_empty_adapter',
  'escape_external_call',
  'escape_json_boundary',
  'escape_hatch',
]);

function constructName(n: IrNode): string {
  return n.metadata.subkind ?? n.kind;
}

/** Lenses declared on this construct type in the presentation model. */
export function lensesForNode(
  node: IrNode,
  presentation: PresentationModel | null
): string[] {
  if (!presentation) return [];
  const name = constructName(node);
  return presentation.constructs[name]?.lenses ?? [];
}

export function nodeHasLens(
  node: IrNode,
  lens: string,
  presentation: PresentationModel | null
): boolean {
  return lensesForNode(node, presentation).includes(lens);
}

/** Escape-hatch / critical diagnostics attached to a node. */
export function nodeHasCriticalDiagnostic(
  node: IrNode,
  diags: Diagnostic[]
): boolean {
  return diags.some((d) => {
    const onNode =
      d.node_id === node.id ||
      (d.node_name != null && d.node_name === node.name);
    if (!onNode) return false;
    const code = (d.code ?? '').toLowerCase();
    if (ESCAPE_CODES.has(code) || code.startsWith('escape_')) return true;
    // High-severity errors also count as review-critical
    if (d.severity === 'Error' || d.severity === 'error') return true;
    return false;
  });
}

/**
 * Critical for the "Critical only" filter:
 * - construct has presentation lens `critical`, or
 * - node has escape-hatch / error diagnostic, or
 * - annotations include names that layers mark via lens (already in presentation)
 */
export function isCriticalNode(
  node: IrNode,
  presentation: PresentationModel | null,
  diags: Diagnostic[]
): boolean {
  if (nodeHasLens(node, 'critical', presentation)) return true;
  if (nodeHasCriticalDiagnostic(node, diags)) return true;
  // Layer-provided infrastructure is not "critical" for review focus
  if (node.metadata.annotations.includes('layer-provided')) return false;
  return false;
}

export function countCritical(
  graph: IrGraph,
  presentation: PresentationModel | null,
  diags: Diagnostic[]
): number {
  return graph.nodes.filter(
    (n) =>
      n.kind !== 'Solution' &&
      !n.metadata.annotations.includes('layer-provided') &&
      isCriticalNode(n, presentation, diags)
  ).length;
}
